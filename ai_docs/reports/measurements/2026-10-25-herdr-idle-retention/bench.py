#!/usr/bin/env python3
"""Herdr control-call latency and server CPU against the number and kind of tabs.

Starts a PRIVATE Herdr server per scenario (its own HOME, XDG directories and socket under
BENCH_ROOT), fills one workspace with tabs, and measures:

* server CPU (utime+stime from /proc over CPU_WINDOW seconds, as % of one core) and threads;
* latency of workspace list / tab list / pane list / pane process-info / pane read;
* `pane list` from 8 concurrent callers.

It never touches another server: every call carries HERDR_SOCKET_PATH for the private one.

Modes (one JSON row per scenario on stdout):

    bench.py tabs                     idle bash tabs: COUNTS (1,32,64,128,192) x WORKERS (default,16)
    bench.py busy [COMMAND]           TABS (128) tabs, some running COMMAND (default: a 5 Hz full-screen
                                      redraw); BUSY lists how many (0,16,64)
    bench.py client                   TABS (128) tabs, 16 redrawing, without and then with a TUI client
                                      attached (needs `script` from util-linux)

HERDR names the herdr executable (default: `herdr` on PATH).
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from typing import IO

# Resolved against the caller's PATH: the private server and its calls get a minimal one.
HERDR = shutil.which(os.environ.get("HERDR", "herdr")) or os.environ.get("HERDR", "herdr")
ROOT = os.environ.get("BENCH_ROOT", "/tmp/herdr-bench")
CPU_WINDOW = float(os.environ.get("CPU_WINDOW", "15"))
SAMPLES = int(os.environ.get("SAMPLES", "15"))
TABS = int(os.environ.get("TABS", "128"))
TICK = os.sysconf("SC_CLK_TCK")
REDRAW = "while :; do printf '\\033[H\\033[2J'; seq 1 40; sleep 0.2; done"


def env_for(tag: str, workers: str) -> dict[str, str]:
    base = f"{ROOT}/{tag}"
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": f"{base}/home",
        "XDG_RUNTIME_DIR": f"{base}/run",
        "XDG_CONFIG_HOME": f"{base}/home/.config",
        "XDG_STATE_HOME": f"{base}/home/.state",
        "XDG_DATA_HOME": f"{base}/home/.data",
        "HERDR_SOCKET_PATH": f"{base}/herdr.sock",
        "SHELL": "/bin/bash",
        "TERM": "xterm-256color",
    }
    if workers != "default":
        env["TOKIO_WORKER_THREADS"] = workers
    for directory in ("home", "run"):
        os.makedirs(f"{base}/{directory}", mode=0o700, exist_ok=True)
    return env


def call(env: dict[str, str], *args: str) -> tuple[dict[str, object], float]:
    """Run one herdr CLI call; return its JSON result object (empty if none) and seconds taken."""
    started = time.monotonic()
    done = subprocess.run([HERDR, *args], env=env, capture_output=True, text=True, timeout=60)
    elapsed = time.monotonic() - started
    if done.returncode != 0:
        raise RuntimeError(f"{args}: {done.stderr.strip()}")
    parsed: object = json.loads(done.stdout) if done.stdout.strip().startswith("{") else {}
    document = parsed if isinstance(parsed, dict) else {}
    inner = document.get("result", document)
    return (inner if isinstance(inner, dict) else {}), elapsed


def field(document: dict[str, object], *path: str) -> str:
    value: object = document
    for key in path:
        value = value.get(key) if isinstance(value, dict) else None
    if not isinstance(value, str):
        raise RuntimeError(f"no {'.'.join(path)} in {document}")
    return value


def pane_ids(env: dict[str, str], workspace: str) -> list[str]:
    panes = call(env, "pane", "list", "--workspace", workspace)[0].get("panes")
    if not isinstance(panes, list):
        raise RuntimeError("pane list returned no panes")
    return [pane["pane_id"] for pane in panes if isinstance(pane, dict) and isinstance(pane.get("pane_id"), str)]


def cpu_ticks(pid: int) -> int:
    with open(f"/proc/{pid}/stat", encoding="utf-8") as handle:
        fields = handle.read().rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def threads(pid: int) -> int:
    with open(f"/proc/{pid}/status", encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("Threads:"):
                return int(line.split()[1])
    return -1


def timings(env: dict[str, str], args: list[str]) -> dict[str, float]:
    values = [call(env, *args)[1] * 1000 for _ in range(SAMPLES)]
    return {"median_ms": round(statistics.median(values), 1), "max_ms": round(max(values), 1)}


def concurrent(env: dict[str, str], args: list[str], callers: int = 8, rounds: int = 3) -> dict[str, float]:
    values: list[float] = []
    with ThreadPoolExecutor(callers) as pool:
        for _ in range(rounds):
            values += [result[1] * 1000 for result in pool.map(lambda _: call(env, *args), range(callers))]
    return {"median_ms": round(statistics.median(values), 1), "max_ms": round(max(values), 1)}


class Server:
    """One private herdr server and the workspace the benchmark fills."""

    def __init__(self, tag: str, workers: str = "default", *, focus: bool = False) -> None:
        self.env = env_for(tag, workers)
        self.log: IO[str] = open(f"{ROOT}/{tag}/server.log", "w", encoding="utf-8")
        self.process = subprocess.Popen(
            [HERDR, "server"], env=self.env, cwd="/tmp", stdout=self.log, stderr=self.log, start_new_session=True
        )
        for _ in range(100):
            try:
                call(self.env, "workspace", "list")
                break
            except (RuntimeError, OSError, ValueError):
                time.sleep(0.1)
        focus_flag = "--focus" if focus else "--no-focus"
        created = call(self.env, "workspace", "create", "--label", "bench", "--cwd", "/tmp", focus_flag)[0]
        self.workspace = field(created, "workspace", "workspace_id")
        self.tabs = 1

    def grow(self, count: int) -> None:
        while self.tabs < count:
            call(self.env, "tab", "create", "--workspace", self.workspace, "--label", f"t{self.tabs}", "--cwd", "/tmp")
            self.tabs += 1
        time.sleep(3)  # let the shells start and settle

    def measure(self, **labels: object) -> dict[str, object]:
        panes = pane_ids(self.env, self.workspace)
        probe = panes[-1]
        before = cpu_ticks(self.process.pid)
        time.sleep(CPU_WINDOW)
        after = cpu_ticks(self.process.pid)
        ws = self.workspace
        row: dict[str, object] = {
            **labels,
            "tabs": len(panes),
            "threads": threads(self.process.pid),
            "cpu_pct": round((after - before) / TICK / CPU_WINDOW * 100, 1),
            "workspace_list": timings(self.env, ["workspace", "list"]),
            "tab_list": timings(self.env, ["tab", "list", "--workspace", ws]),
            "pane_list": timings(self.env, ["pane", "list", "--workspace", ws]),
            "process_info": timings(self.env, ["pane", "process-info", "--pane", probe]),
            "pane_read": timings(self.env, ["pane", "read", probe, "--source", "recent", "--lines", "50"]),
            "pane_list_8_concurrent": concurrent(self.env, ["pane", "list", "--workspace", ws]),
        }
        print(json.dumps(row), flush=True)
        return row

    def stop(self) -> None:
        # Only the process group this script started; the shells it spawned exit on hangup.
        os.killpg(self.process.pid, signal.SIGTERM)
        try:
            self.process.wait(10)
        except subprocess.TimeoutExpired:
            os.killpg(self.process.pid, signal.SIGKILL)
        self.log.close()


def mode_tabs() -> None:
    counts = [int(value) for value in os.environ.get("COUNTS", "1,32,64,128,192").split(",")]
    for workers in os.environ.get("WORKERS", "default,16").split(","):
        server = Server(f"tabs-{workers}", workers)
        try:
            for count in counts:
                server.grow(count)
                server.measure(mode="tabs", workers=workers)
        finally:
            server.stop()


def mode_busy(command: str) -> None:
    for busy in [int(value) for value in os.environ.get("BUSY", "0,16,64").split(",")]:
        server = Server(f"busy-{busy}")
        try:
            server.grow(TABS)
            for pane in pane_ids(server.env, server.workspace)[:busy]:
                call(server.env, "pane", "run", pane, command)
            time.sleep(5)
            server.measure(mode="busy", busy=busy, command=command)
        finally:
            server.stop()


def mode_client() -> None:
    server = Server("client", focus=True)
    client: subprocess.Popen[bytes] | None = None
    try:
        server.grow(TABS)
        for pane in pane_ids(server.env, server.workspace)[:16]:
            call(server.env, "pane", "run", pane, REDRAW)
        time.sleep(5)
        server.measure(mode="client", client=False, busy=16)
        client = subprocess.Popen(
            ["script", "-qfc", f"stty rows 50 cols 200; exec {HERDR}", "/dev/null"],
            env=server.env,
            cwd="/tmp",
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        time.sleep(5)
        server.measure(mode="client", client=client.poll() is None, busy=16)
    finally:
        if client is not None:
            try:
                os.killpg(client.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        server.stop()


def main(argv: list[str]) -> int:
    mode = argv[1] if len(argv) > 1 else "tabs"
    if mode == "tabs":
        mode_tabs()
    elif mode == "busy":
        mode_busy(argv[2] if len(argv) > 2 else REDRAW)
    elif mode == "client":
        mode_client()
    else:
        print(__doc__, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
