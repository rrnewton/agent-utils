"""Reboot-free restore drill: the real host command end to end (#1133 restore-drill-proof).

Real: the built ``agentctl`` binary (``restore-teams`` and the per-registry
``restore`` it calls), a scratch registry with planted records, profiles and
``restore.json``, a scratch terminal server in a named test session on its
own socket, a scratch slot project (real Git repositories, the tracked
wrkslots launcher named by absolute path), the per-boot report files, exit
codes, queued briefs read back from each lane's queue, and slot ownership
read back through ``wrkslots status``.

Stubbed: harness processes only. Every lane runs one compiled fake Muse
(a custom harness must be an ELF image, so a shell script cannot stand in)
selected with ``AGENTCTL_MUSE_BIN``. It prints an idle composer frame,
records its environment and argv per session, and stays alive.

Boot identity split, stated plainly: the product reads the current boot id
from ``/proc/sys/kernel/random/boot_id`` and offers no environment or config
override, so a black-box binary run cannot be handed a fake current boot.
The drill plants the disk state a real reboot leaves behind -- records whose
pinned identities carry a previous boot id -- while the host reports the
current one. Pure ``Host`` injection (a scripted boot id, clock and command
answers) remains the Rust unit suites' coverage; this drill exercises the
binary, its exit classes and its report replay against real processes.

Environment survival: a profile's ``env`` entries are tab-creation entries,
so an env-carrying revive takes a fresh tab instead of reusing the
server-restored shell; the registry ``launch_env`` is typed into the pane
on every start and revive. The drill therefore pins profile env at first
start and both profile env and ``launch_env`` (``BASH_ENV`` and the
``PATH`` prefix) after a revive into a fresh pane.

Isolation: the scratch server is reached only as ``herdr --session NAME``
(through a wrapper given as ``--herdr-bin``) with ``HERDR_SOCKET_PATH`` set
to that session's own socket in every subprocess environment. The live
server, live registries, units and bridges are never addressed. Simulated
reboot kills stub harnesses by literal pid read from their records, each
re-checked with ``ps`` immediately before the signal, then stops and
restarts only the scratch session. Assertions read state and reports only,
never probe timings. When a prerequisite (terminal server, C compiler, Git,
wrkslots launcher) is absent the drill skips with that reason; it never
passes silently. Teardown stops and deletes the scratch session and
asserts it is gone.
"""

from __future__ import annotations

import io
import json
import os
import shutil
import subprocess
import threading
import time
import uuid
from collections.abc import Callable
from pathlib import Path
from types import TracebackType
from typing import cast

import pytest

from tests.rust_binary import rust_agentctl


REPO_ROOT = Path(__file__).resolve().parents[2]
WRKSLOTS_BIN = REPO_ROOT / "common/bin/wrkslots"
PREVIOUS_BOOT = "00000000-1111-2222-3333-444444444444"
CERT_FETCH = "drill-fetch-user-certificate"
PROFILE_VALUE = "profile-env-1133"
FAKE_NAME = "drill-fake-muse"

FAKE_SOURCE = r"""
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
extern char **environ;
int main(int argc, char **argv) {
    const char *dir = getenv("FAKE_HARNESS_LOG_DIR");
    const char *session = argc > 0 ? argv[argc - 1] : "unknown";
    if (dir) {
        char path[4096];
        snprintf(path, sizeof(path), "%s/%s.log", dir, session);
        FILE *f = fopen(path, "a");
        if (f) {
            for (char **e = environ; *e; e++) fprintf(f, "%s\n", *e);
            fprintf(f, "ARGV:");
            for (int i = 0; i < argc; i++) fprintf(f, " [%s]", argv[i]);
            fprintf(f, "\n---\n");
            fclose(f);
        }
    }
    printf("Muse Code 1.4.16\n");
    printf("drill fake harness ready\n");
    printf("\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\n");
    printf("\xe2\x9d\xaf\n");
    printf("\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\n");
    printf("  fakemodel \xc2\xb7 xhigh \xc2\xb7 /tmp \xc2\xb7 YOLO\n");
    fflush(stdout);
    for (;;) pause();
    return 0;
}
"""


def _host_boot_id() -> str:
    return Path("/proc/sys/kernel/random/boot_id").read_text(encoding="utf-8").strip()


def _require_prerequisites() -> tuple[Path, Path]:
    """The terminal-server and compiler paths, or a skip naming what is absent."""
    herdr = shutil.which("herdr")
    if herdr is None:
        pytest.skip("restore drill needs the herdr binary on PATH")
    compiler = shutil.which("cc")
    if compiler is None:
        pytest.skip("restore drill needs a C compiler (cc) for its fake harness")
    if shutil.which("git") is None:
        pytest.skip("restore drill needs git for its scratch slot project")
    if shutil.which("openssl") is None:
        pytest.skip("restore drill needs openssl for a parseable scratch certificate")
    if not WRKSLOTS_BIN.is_file():
        pytest.skip(f"restore drill needs the wrkslots launcher at {WRKSLOTS_BIN}")
    try:
        boot = _host_boot_id()
    except OSError:
        pytest.skip("restore drill needs a readable /proc boot id")
    if boot == PREVIOUS_BOOT:
        pytest.skip("restore drill previous-boot plant collides with the host boot id")
    return Path(herdr), Path(compiler)


def _certificate_pem(directory: Path) -> bytes:
    """A real self-signed PEM certificate: restore parses notAfter (#1137)."""
    key = directory / "cert-key.pem"
    cert = directory / "cert-generated.pem"
    subprocess.run(
        [
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", str(key), "-out", str(cert), "-days", "3650",
            "-subj", "/CN=restore-drill",
        ],
        check=True, capture_output=True, timeout=120,
    )
    return cert.read_bytes()


def _build_fake_harness(directory: Path, compiler: Path) -> Path:
    source = directory / "drill-fake.c"
    binary = directory / FAKE_NAME
    source.write_text(FAKE_SOURCE, encoding="utf-8")
    subprocess.run(
        [str(compiler), "-O2", "-o", str(binary), str(source)],
        check=True, capture_output=True, text=True, timeout=120,
    )
    binary.chmod(0o755)
    assert binary.read_bytes()[:4] == b"\x7fELF"
    return binary


def _ps_comm(pid: int) -> str:
    result = subprocess.run(
        ["ps", "-o", "comm=", "-p", str(pid)],
        capture_output=True, text=True, timeout=30,
    )
    return result.stdout.strip()


class Drill:
    """One scratch world: registry, slot project, terminal server, fake harness."""

    def __init__(self, root: Path, *, slot_representation: str) -> None:
        self.herdr, compiler = _require_prerequisites()
        self.agentctl = rust_agentctl()
        self.root = root
        self.project = root / "project"
        self.registry = self.project / ".agentctl"
        self.reports = root / "reports"
        self.certificate = root / "cert.pem"
        self.log_dir = root / "fake-logs"
        self.tools_dir = root / "drill-tools"
        self.env_file = root / "launch-env.sh"
        self.session = f"restore-drill-{os.getpid()}-{uuid.uuid4().hex[:8]}"
        self.socket: Path | None = None
        self.server: subprocess.Popen[bytes] | None = None
        self._server_log: io.BufferedWriter | None = None
        self.image_slots: list[str] = []
        self.fake = _build_fake_harness(root, compiler)
        self._prepare(slot_representation)

    # -- scratch state ----------------------------------------------------

    def _prepare(self, slot_representation: str) -> None:
        self.registry.mkdir(parents=True)
        self.reports.mkdir()
        self.log_dir.mkdir()
        self.tools_dir.mkdir()
        (self.root / "xdg-data").mkdir()
        (self.root / "xdg-state").mkdir()
        self.env_file.write_text("# drill launch env\n", encoding="utf-8")
        subprocess.run(
            ["git", "init", "-q", str(self.project)],
            check=True, capture_output=True, text=True, timeout=60,
        )
        (self.project / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        self._prepare_slot_project(slot_representation)
        wrapper = self.root / "herdr-scratch"
        wrapper.write_text(
            f'#!/bin/sh\ncase "$1" in --version|-V) exec {self.herdr} "$@";; esac\n'
            f'exec {self.herdr} --session {self.session} "$@"\n',
            encoding="utf-8",
        )
        wrapper.chmod(0o755)
        self.wrapper = wrapper
        profiles: dict[str, object] = {
            "schema": "agentctl-profiles/v1",
            "workspace": "drill-ws",
            "wrkslots": str(WRKSLOTS_BIN),
            "launch_env": {
                "env_file": str(self.env_file),
                "path_prepend": [str(self.tools_dir)],
            },
            "profiles": {
                "fakework": {
                    "harness": "muse", "mode": "interactive",
                    "reasoning_effort": "high",
                    "env": {"FAKE_HARNESS_LOG_DIR": str(self.log_dir)},
                },
                "profwork": {
                    "harness": "muse", "mode": "interactive",
                    "reasoning_effort": "xhigh",
                    "env": {
                        "FAKE_HARNESS_LOG_DIR": str(self.log_dir),
                        "DRILL_PROFILE_VALUE": PROFILE_VALUE,
                    },
                },
            },
        }
        (self.registry / "profiles.json").write_text(
            json.dumps(profiles), encoding="utf-8",
        )
        (self.registry / "profiles.json").chmod(0o600)
        self.registry.chmod(0o700)
        restore: dict[str, object] = {
            "schema": "agentctl-restore/v1",
            "coordinator": "coord",
            "startup_delay_seconds": 0,
            "herdr_wait_seconds": 60,
            "report_dir": str(self.reports),
            "certificate_file": str(self.certificate),
            "certificate_fetch": CERT_FETCH,
            "resume_brief": (
                "[drill] {{agent}} restored after boot {{boot_id}}; "
                "coordinator {{coordinator}}; report {{report}}"
            ),
        }
        (self.registry / "restore.json").write_text(
            json.dumps(restore), encoding="utf-8",
        )
        (self.registry / "restore.json").chmod(0o600)
        self.cert_pem = _certificate_pem(self.root)
        self.certificate.write_bytes(self.cert_pem)

    def _prepare_slot_project(self, slot_representation: str) -> None:
        remote = self.root / "remote.git"
        repository = self.project / "repo"
        subprocess.run(
            ["git", "init", "--bare", "--initial-branch=main", str(remote)],
            check=True, capture_output=True, text=True, timeout=60,
        )
        subprocess.run(
            ["git", "clone", str(remote), str(repository)],
            check=True, capture_output=True, text=True, timeout=60,
        )

        def git(*args: str) -> None:
            subprocess.run(
                ["git", "-C", str(repository), *args],
                check=True, capture_output=True, text=True, timeout=60,
            )

        git("config", "user.name", "Drill Test")
        git("config", "user.email", "drill@example.invalid")
        git("config", "gc.autoDetach", "false")
        (repository / "seed.txt").write_text("seed\n", encoding="utf-8")
        git("add", "seed.txt")
        git("commit", "-m", "seed")
        git("push", "-u", "origin", "main")
        # Removal is never exercised; ownership proofs use process identity.
        # The liveness command still answers from an explicit fixture file.
        liveness = self.project / "liveness.py"
        liveness.write_text(
            "#!/usr/bin/env python3\n"
            "import os, pathlib\n"
            "root = pathlib.Path(os.environ['WRKSLOTS_PROJECT_ROOT'])\n"
            "state = (root / 'liveness-result').read_text().strip()\n"
            "raise SystemExit({'dead': 0, 'alive': 1}.get(state, 2))\n",
            encoding="utf-8",
        )
        liveness.chmod(0o755)
        (self.project / "liveness-result").write_text("alive\n", encoding="utf-8")
        created = self.wrkslots(
            "init", str(self.project), "--worktrees-dir", "worktrees",
            "--liveness-command", "liveness.py",
            "--sandbox-isolation", "cgroup",
            "--slot-representation", slot_representation,
        )
        assert created.returncode == 0, created.stderr

    # -- subprocess plumbing ------------------------------------------------

    def env(self) -> dict[str, str]:
        """An environment that can only reach the scratch session and state."""
        env = dict(os.environ)
        for name in (
            "HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID",
            "HERDR_WORKSPACE_ID", "HERDR_BIN_PATH", "HERDR_ENV",
            "WRKSLOTS_MACHINE",
        ):
            env.pop(name, None)
        if self.socket is not None:
            env["HERDR_SOCKET_PATH"] = str(self.socket)
        env["AGENTCTL_MUSE_BIN"] = str(self.fake)
        env["AGENTCTL_WRKSLOTS_BIN"] = str(WRKSLOTS_BIN)
        env["XDG_DATA_HOME"] = str(self.root / "xdg-data")
        env["XDG_STATE_HOME"] = str(self.root / "xdg-state")
        env["FAKE_HARNESS_LOG_DIR"] = str(self.log_dir)
        return env

    def run(
        self, argv: list[str], *, timeout: float = 240,
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            argv, capture_output=True, text=True, env=self.env(), timeout=timeout,
        )

    def herdr_run(
        self, *args: str, timeout: float = 30,
    ) -> subprocess.CompletedProcess[str]:
        return self.run(
            [str(self.herdr), "--session", self.session, *args], timeout=timeout,
        )

    def agentctl_run(
        self, *args: str, timeout: float = 240,
    ) -> subprocess.CompletedProcess[str]:
        return self.run(
            [
                str(self.agentctl), "--registry", str(self.registry),
                "--herdr-bin", str(self.wrapper), *args,
            ],
            timeout=timeout,
        )

    def wrkslots(
        self, *args: str, timeout: float = 900,
    ) -> subprocess.CompletedProcess[str]:
        # Slot/image work scans the live host; under validation load the
        # 240 s default expires before the refusal under test is reached.
        return self.run([str(WRKSLOTS_BIN), *args], timeout=timeout)

    def wrkslots_project(
        self, *args: str, timeout: float = 900,
    ) -> subprocess.CompletedProcess[str]:
        return self.wrkslots(
            "--project-root", str(self.project), *args, timeout=timeout,
        )

    # -- scratch terminal server --------------------------------------------

    def _session_socket(self) -> Path | None:
        listed = self.run(
            [str(self.herdr), "session", "list", "--json"], timeout=30,
        )
        document = cast(dict[str, object], json.loads(listed.stdout))
        sessions = cast(list[dict[str, object]], document["sessions"])
        for entry in sessions:
            if entry.get("name") == self.session:
                return Path(cast(str, entry["socket_path"]))
        return None

    def start_server(self) -> None:
        log = (self.root / "herdr-server.log").open("ab")
        self._server_log = log
        self.server = subprocess.Popen(
            [str(self.herdr), "--session", self.session, "server"],
            env=self.env(), stdout=log, stderr=log, stdin=subprocess.DEVNULL,
        )
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            answer = self.herdr_run("pane", "list")
            if '"result"' in answer.stdout and '"error"' not in answer.stdout:
                break
            time.sleep(0.2)
        else:
            raise AssertionError(
                "scratch terminal server never answered: "
                + (self.root / "herdr-server.log").read_text(encoding="utf-8")[-1000:]
            )
        socket = self._session_socket()
        assert socket is not None, "scratch session missing from session list"
        assert self.session in str(socket), socket
        live_default = Path.home() / ".config/herdr/herdr.sock"
        assert socket != live_default, socket
        self.socket = socket

    def stop_server(self) -> None:
        assert self.server is not None
        self.herdr_run("server", "stop")
        try:
            self.server.wait(timeout=15)
        except subprocess.TimeoutExpired:
            assert _ps_comm(self.server.pid) != "", "scratch server pid vanished"
            self.server.terminate()
            self.server.wait(timeout=10)
        self.server = None
        if self._server_log is not None:
            self._server_log.close()
            self._server_log = None

    def panes(self) -> list[dict[str, object]]:
        answer = self.herdr_run("pane", "list")
        document = cast(dict[str, object], json.loads(answer.stdout))
        result = cast(dict[str, object], document["result"])
        return cast(list[dict[str, object]], result["panes"])

    # -- lanes ----------------------------------------------------------------

    def create_slot(self, slot: str, agent: str) -> None:
        created = self.wrkslots_project(
            "create", slot, "--slot-type", "agent", "--coordinator-authorized",
            "--agent", agent, "--task", "drill-task",
            "--purpose", "restore drill slot",
            "--coordinator-pid", str(os.getpid()),
            "--repo", "product=repo", "--branch", f"product=drill/{slot}",
        )
        assert created.returncode == 0, created.stderr
        status = self.wrkslots_project("image", "status")
        if f"/{slot} at " in status.stdout:
            self.image_slots.append(slot)

    def start_lane(
        self, name: str, profile: str, session: str, *,
        slot: str | None = None, cwd: Path | None = None,
    ) -> dict[str, object]:
        argv = [
            "start", name, "--profile", profile,
            "--cwd", str(cwd or self.project), "--resume", session,
        ]
        if slot is not None:
            argv += [
                "--slot", slot, "--slot-isolation", "cgroup",
                "--slot-project", str(self.project),
            ]
        started = self.agentctl_run(*argv, timeout=300)
        assert started.returncode == 0, started.stderr
        return cast(dict[str, object], json.loads(started.stdout))

    def designate_coordinator(self) -> None:
        designated = self.agentctl_run("coordinator", "set", "coord")
        assert designated.returncode == 0, designated.stderr

    def record(self, name: str) -> dict[str, object]:
        path = self.registry / "agents" / name / "agent.json"
        return cast(dict[str, object], json.loads(path.read_text(encoding="utf-8")))

    def harness_pid(self, name: str) -> int:
        record = self.record(name)
        identity = cast(
            dict[str, object],
            record.get("custom_process_identity") or record["harness_identity"],
        )
        return cast(int, identity["pid"])

    def launches(self, session: str) -> list[str]:
        """One recorded environment block per fake-harness launch of `session`."""
        path = self.log_dir / f"{session}.log"
        if not path.is_file():
            return []
        blocks = path.read_text(encoding="utf-8").split("---\n")
        return [block for block in blocks if "ARGV:" in block]

    def queued(self, name: str) -> list[str]:
        inbox = self.registry / "agents" / name / "queue" / "inbox"
        if not inbox.is_dir():
            return []
        return sorted(path.name for path in inbox.glob("*.json"))

    def slot_owner_pid(self, slot: str) -> int | None:
        status = self.wrkslots_project(
            "--wait-lock", "30", "status", "--slot", slot, "--format", "json",
        )
        assert status.returncode == 0, status.stderr
        document = cast(dict[str, object], json.loads(status.stdout))
        active = cast(list[dict[str, object]], document["active"])
        row = next(entry for entry in active if entry.get("slot") == slot)
        owner = row.get("owner")
        if not isinstance(owner, dict):
            return None
        pid = cast(dict[str, object], owner).get("pid")
        return pid if isinstance(pid, int) else None

    # -- simulated reboot -----------------------------------------------------

    def kill_harness(self, name: str) -> None:
        """Kill one stub harness by its literal recorded pid, re-checked first."""
        pid = self.harness_pid(name)
        assert _ps_comm(pid) == FAKE_NAME, (name, pid, _ps_comm(pid))
        os.kill(pid, 9)

    def plant_previous_boot(self, name: str) -> None:
        """Leave `name`'s record as a real reboot leaves it: previous boot id."""
        path = self.registry / "agents" / name / "agent.json"
        document = cast(dict[str, object], json.loads(path.read_text(encoding="utf-8")))
        for key in ("custom_process_identity", "harness_identity"):
            identity = document.get(key)
            if isinstance(identity, dict):
                cast(dict[str, object], identity)["boot_id"] = PREVIOUS_BOOT
        path.write_text(json.dumps(document), encoding="utf-8")
        path.chmod(0o600)

    def reboot(
        self, names: list[str], *, while_down: Callable[[], None] | None = None,
    ) -> dict[str, str]:
        """Kill stubs, restart only the scratch server; return restored panes.

        The returned map is pane id to cwd for every shell the restarted
        server auto-restored, read before any restore runs: the reuse proof
        compares revived pane ids against it. `while_down` runs with the
        scratch server stopped (no pane shell alive), for plants a live
        shell would block, such as unmounting a slot image.
        """
        for name in names:
            self.kill_harness(name)
        self.stop_server()
        for name in names:
            self.plant_previous_boot(name)
        if while_down is not None:
            while_down()
        self.start_server()
        # Let auto-restored presentations settle before restore proofs read
        # them: under validation load a pane still settling reads as an
        # owned presentation changed during the proof and refuses a lane
        # that standalone runs revive. Poll until the pane set is stable.
        restored: dict[str, str] = {}
        deadline = time.monotonic() + 60
        while True:
            current: dict[str, str] = {}
            for pane in self.panes():
                current[cast(str, pane["pane_id"])] = cast(
                    str, pane.get("cwd", ""),
                )
                assert pane.get("agent") is None, pane
            if current == restored and current:
                break
            restored = current
            assert time.monotonic() < deadline, restored
            time.sleep(1.0)
        return restored

    def restore_teams(
        self, *, cert_wait_seconds: int,
    ) -> tuple[int, dict[str, object], str]:
        completed = self.run(
            [
                str(self.agentctl), "--herdr-bin", str(self.wrapper),
                "restore-teams", "--registry", str(self.registry),
                "--cert-wait-seconds", str(cert_wait_seconds),
                "--startup-timeout", "120",
            ],
            timeout=600,
        )
        document = cast(dict[str, object], json.loads(completed.stdout))
        return completed.returncode, document, completed.stderr

    def report_file(self) -> dict[str, object]:
        path = self.reports / f"{_host_boot_id()}.json"
        return cast(dict[str, object], json.loads(path.read_text(encoding="utf-8")))

    # -- teardown ---------------------------------------------------------------

    def close(self) -> None:
        """Leave no process, socket, session or mount behind; assert it."""
        names: list[str] = []
        agents = self.registry / "agents"
        if agents.is_dir():
            names = [
                entry.name for entry in agents.iterdir()
                if (entry / "agent.json").is_file()
            ]
        for name in names:
            try:
                pid = self.harness_pid(name)
            except (OSError, KeyError, ValueError):
                continue
            if _ps_comm(pid) == FAKE_NAME:
                os.kill(pid, 9)
        if self.server is not None:
            self.stop_server()
        for slot in self.image_slots:
            self.wrkslots_project("image", "unmount", slot)
        deleted = self.run(
            [str(self.herdr), "session", "delete", self.session], timeout=30,
        )
        assert deleted.returncode == 0, deleted.stderr
        assert self._session_socket() is None, self.session
        for name in names:
            try:
                pid = self.harness_pid(name)
            except (OSError, KeyError, ValueError):
                continue
            deadline = time.monotonic() + 5
            while _ps_comm(pid) != "" and time.monotonic() < deadline:
                time.sleep(0.1)
            assert _ps_comm(pid) == "", (name, pid)

    def __enter__(self) -> Drill:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()


def _rows_by_name(document: dict[str, object]) -> dict[str, dict[str, object]]:
    rows = cast(list[dict[str, object]], document["rows"])
    return {cast(str, row["name"]): row for row in rows}


def _registry_report(document: dict[str, object]) -> dict[str, object]:
    registries = cast(list[dict[str, object]], document["registries"])
    report = registries[0].get("report")
    assert isinstance(report, dict), document
    return cast(dict[str, object], report)


def test_restore_drill_happy_path_fresh_pane_slot_and_rerun(tmp_path: Path) -> None:
    """Four lanes back after one command; a rerun replays and relaunches nothing.

    The slot lane uses a plain worktree slot here (owner transfer is the
    behaviour under test); the image-backed bind check has its own variant
    below. Every drill profile carries environment, so revive must not
    reuse the auto-restored shell (its tab environment died with the old
    shell); each lane relaunches in a fresh pane and both the profile
    value and the registry launch environment are present again.
    """
    sessions = {
        "coord": "drill-session-coord",
        "cheap": "drill-session-cheap",
        "prof": "drill-session-prof",
        "slotlane": "drill-session-slot",
    }
    with Drill(tmp_path, slot_representation="worktree") as drill:
        drill.start_server()
        drill.create_slot("slot01", "slotlane")
        drill.start_lane("coord", "fakework", sessions["coord"])
        drill.start_lane("cheap", "fakework", sessions["cheap"])
        drill.start_lane("prof", "profwork", sessions["prof"])
        drill.start_lane(
            "slotlane", "fakework", sessions["slotlane"], slot="slot01",
        )
        drill.designate_coordinator()
        # Profile env is tab-creation env: present in the first launch,
        # with the profile's declared reasoning effort carried on argv.
        prof_first = drill.launches(sessions["prof"])[0]
        assert f"DRILL_PROFILE_VALUE={PROFILE_VALUE}" in prof_first
        assert "reasoning-effort" in prof_first and "xhigh" in prof_first
        old_panes = {
            name: cast(str, drill.record(name)["pane_id"]) for name in sessions
        }
        old_slot_pid = drill.harness_pid("slotlane")
        assert drill.slot_owner_pid("slot01") == old_slot_pid

        restored = drill.reboot(list(sessions))
        # The restarted server auto-restored an idle shell in every pane,
        # under the same pane ids the records hold, reporting no agent:
        # exactly the state revive reuses in place.
        for name, pane in old_panes.items():
            assert pane in restored, (name, restored)
        drill.certificate.unlink()
        timer = threading.Timer(
            2.0, lambda: drill.certificate.write_bytes(drill.cert_pem),
        )
        timer.start()
        try:
            code, document, table = drill.restore_teams(cert_wait_seconds=60)
        finally:
            timer.cancel()

        assert code == 0, table
        assert document["complete"] is True
        assert document["refused"] == 0
        rows = _rows_by_name(document)
        report = _registry_report(document)
        assert report["restart_detected"] is True
        assert report["previous_boots"] == [PREVIOUS_BOOT]
        assert drill.report_file()["complete"] is True
        new_panes: dict[str, str] = {}
        for name, session in sessions.items():
            assert rows[name]["action"] == "revived", rows[name]
            assert rows[name]["replayed"] is False
            # Env-carrying launches never reuse the restored shell: a fresh
            # pane receives the tab environment the shell lost.
            new_panes[name] = cast(str, drill.record(name)["pane_id"])
            assert new_panes[name] != old_panes[name], name
            assert _ps_comm(drill.harness_pid(name)) == FAKE_NAME, name
            assert len(drill.launches(session)) == 2, name
        # Profile and launch environment both survive the revive, and the
        # declared reasoning effort is carried through it.
        prof_after = drill.launches(sessions["prof"])[1]
        assert f"DRILL_PROFILE_VALUE={PROFILE_VALUE}" in prof_after
        assert "reasoning-effort" in prof_after and "xhigh" in prof_after
        assert f"BASH_ENV={drill.env_file}" in prof_after
        assert f"PATH={drill.tools_dir}:" in prof_after
        # The slot is owned by the new harness pid, read through wrkslots.
        new_slot_pid = drill.harness_pid("slotlane")
        assert new_slot_pid != old_slot_pid
        assert drill.slot_owner_pid("slot01") == new_slot_pid
        agent_rows = {
            cast(str, row["name"]): row
            for row in cast(list[dict[str, object]], report["agents"])
        }
        slot_owner = cast(dict[str, object], agent_rows["slotlane"]["slot_owner"])
        assert slot_owner["action"] == "transferred", report
        assert slot_owner["owner_pid"] == new_slot_pid
        # One brief and one catch-up per revived lane, queued exactly once.
        boot8 = _host_boot_id()[:8]
        for name in ("cheap", "prof", "slotlane"):
            assert drill.queued(name) == [
                f"restore-brief-{boot8}-{name}.json",
                f"restore-catchup-{boot8}-{name}.json",
            ], name
            brief = json.loads(
                (
                    drill.registry / "agents" / name / "queue" / "inbox"
                    / f"restore-brief-{boot8}-{name}.json"
                ).read_text(encoding="utf-8"),
            )
            assert name in cast(str, brief["text"])
            assert _host_boot_id() in cast(str, brief["text"])
        assert drill.queued("coord") == [f"restore-{boot8}.json"]

        # An immediate rerun is a truthful no-op at the same exit class:
        # replayed rows, no second launch, no second brief.
        code, document, table = drill.restore_teams(cert_wait_seconds=5)
        assert code == 0, table
        rows = _rows_by_name(document)
        for name, session in sessions.items():
            assert rows[name]["action"] == "revived", rows[name]
            assert rows[name]["replayed"] is True, rows[name]
            assert drill.record(name)["pane_id"] == new_panes[name], name
            assert len(drill.launches(session)) == 2, name
        for name in ("cheap", "prof", "slotlane"):
            assert drill.queued(name) == [
                f"restore-brief-{boot8}-{name}.json",
                f"restore-catchup-{boot8}-{name}.json",
            ], name
        assert "(replayed)" in table


def test_restore_drill_certificate_never_appears_gates_at_75(tmp_path: Path) -> None:
    """No certificate within the bound: exit 75, remedy named, nothing launched."""
    sessions = {"coord": "drill-session-coord", "lane": "drill-session-lane"}
    with Drill(tmp_path, slot_representation="worktree") as drill:
        drill.start_server()
        drill.start_lane("coord", "fakework", sessions["coord"])
        drill.start_lane("lane", "fakework", sessions["lane"])
        drill.designate_coordinator()
        before = {name: drill.record(name) for name in sessions}
        drill.reboot(list(sessions))
        drill.certificate.unlink()

        code, document, table = drill.restore_teams(cert_wait_seconds=1)

        assert code == 75, table
        assert document["complete"] is False
        registries = cast(list[dict[str, object]], document["registries"])
        assert registries[0]["status"] == "certificate-timeout"
        certificate = cast(dict[str, object], registries[0]["certificate"])
        assert certificate["remedy"] == CERT_FETCH
        assert "certificate-timeout" in table
        assert CERT_FETCH in table
        # Nothing that needs the certificate launched: records still hold
        # their dead pre-reboot panes and pids, and no report was written.
        for name, session in sessions.items():
            after = drill.record(name)
            assert after["pane_id"] == before[name]["pane_id"], name
            assert drill.harness_pid(name) == cast(
                int,
                cast(dict[str, object], before[name]["custom_process_identity"])["pid"],
            ), name
            assert len(drill.launches(session)) == 1, name
        assert list(drill.reports.iterdir()) == []


def test_restore_drill_one_unrestorable_lane_refused_at_74(tmp_path: Path) -> None:
    """One unrestorable lane: exit 74 naming it while the others come back.

    The stuck lane's recorded starting directory is removed while the host
    is down, so its revive must refuse before launching anything there.
    """
    sessions = {
        "coord": "drill-session-coord",
        "lane": "drill-session-lane",
        "stuck": "drill-session-stuck",
    }
    with Drill(tmp_path, slot_representation="worktree") as drill:
        drill.start_server()
        stuck_cwd = drill.project / "stuck-cwd"
        stuck_cwd.mkdir()
        drill.start_lane("coord", "fakework", sessions["coord"])
        drill.start_lane("lane", "fakework", sessions["lane"])
        drill.start_lane("stuck", "fakework", sessions["stuck"], cwd=stuck_cwd)
        drill.designate_coordinator()
        drill.reboot(list(sessions))
        # The restored shell still sits in the directory; removing it now
        # leaves the recorded starting directory unavailable to revive.
        shutil.rmtree(stuck_cwd)

        code, document, table = drill.restore_teams(cert_wait_seconds=5)

        assert code == 74, table
        assert document["refused"] == 1
        assert document["refused_lanes"] == [
            {"registry": str(drill.registry), "name": "stuck"},
        ]
        rows = _rows_by_name(document)
        assert rows["stuck"]["action"] == "refused"
        # The refusal names the planted cause; which revive check reports
        # it first depends on how the restored shell's cwd reads back.
        reason = cast(str, rows["stuck"]["reason"])
        assert "cwd" in reason or "starting directory" in reason, reason
        assert "stuck refused" in table
        for name in ("coord", "lane"):
            assert rows[name]["action"] == "revived", rows[name]
            assert _ps_comm(drill.harness_pid(name)) == FAKE_NAME, name
            assert len(drill.launches(sessions[name])) == 2, name
        assert len(drill.launches(sessions["stuck"])) == 1
        assert drill.queued("stuck") == []

        # The rerun keeps the recorded 74 class with replayed rows.
        code, document, table = drill.restore_teams(cert_wait_seconds=5)
        assert code == 74, table
        rows = _rows_by_name(document)
        assert rows["stuck"]["action"] == "refused"
        assert rows["stuck"]["replayed"] is True
        assert len(drill.launches(sessions["lane"])) == 2


def test_restore_drill_missing_slot_bind_refuses_only_that_lane(
    tmp_path: Path,
) -> None:
    """Bare path (D3): an unmounted image slot refuses its lane; others revive.

    The slot image is unmounted with the real slot manager and its mount
    point is left holding a stray file, so the remount every wrkslots
    command attempts is refused and ``image status`` reports the slot image
    not mounted. Restore must refuse that lane before revive instead of
    starting it on the bare mount point.
    """
    sessions = {
        "coord": "drill-session-coord",
        "lane": "drill-session-lane",
        "slotlane": "drill-session-slot",
    }
    with Drill(tmp_path, slot_representation="image") as drill:
        drill.start_server()
        drill.create_slot("slot01", "slotlane")
        assert "slot01" in drill.image_slots
        drill.start_lane("coord", "fakework", sessions["coord"])
        drill.start_lane("lane", "fakework", sessions["lane"])
        drill.start_lane(
            "slotlane", "fakework", sessions["slotlane"], slot="slot01",
        )
        drill.designate_coordinator()
        old_slot_pid = drill.harness_pid("slotlane")
        assert drill.slot_owner_pid("slot01") == old_slot_pid
        slot_dir = drill.project / "worktrees" / "slot01"

        def unmount_and_block() -> None:
            # No pane shell is alive while the scratch server is down, so
            # the unmount the restored shell's cwd would refuse succeeds.
            unmounted = drill.wrkslots_project("image", "unmount", "slot01")
            assert unmounted.returncode == 0, unmounted.stderr
            (slot_dir / "STRAY.txt").write_text("stray\n", encoding="utf-8")

        drill.reboot(list(sessions), while_down=unmount_and_block)

        code, document, table = drill.restore_teams(cert_wait_seconds=5)

        assert code == 74, table
        assert document["refused_lanes"] == [
            {"registry": str(drill.registry), "name": "slotlane"},
        ]
        rows = _rows_by_name(document)
        assert rows["slotlane"]["action"] == "refused"
        assert "slot bind not verified" in cast(str, rows["slotlane"]["reason"])
        report = _registry_report(document)
        agent_rows = {
            cast(str, row["name"]): row
            for row in cast(list[dict[str, object]], report["agents"])
        }
        assert cast(
            dict[str, object], agent_rows["slotlane"]["slot_bind"],
        )["action"] == "refused"
        for name in ("coord", "lane"):
            assert rows[name]["action"] == "revived", rows[name]
            assert _ps_comm(drill.harness_pid(name)) == FAKE_NAME, name
        # The refused lane never launched and its slot kept the dead owner.
        assert len(drill.launches(sessions["slotlane"])) == 1
        assert drill.slot_owner_pid("slot01") == old_slot_pid
        assert drill.queued("slotlane") == []
        # Leave the mount point mountable again for teardown.
        (slot_dir / "STRAY.txt").unlink()
