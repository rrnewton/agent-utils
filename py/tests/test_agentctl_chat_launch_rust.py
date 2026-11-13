"""Integration: Rust `agentctl chat launch` with fake providers only.

M2 rust-primary-agentctl-chat-launch (generation B: the Rust bridge).
Every external system is a fixture: a fake `herdr` executable answers
pane queries from files, a fake `codex` harness is a shell script, the
chat provider is the deterministic `chat-subscription-fake-plugin`,
and the registry/state live in temporary directories. Nothing live
is touched. The Python polling bridge's launch is the behavioural
reference for refusals and lifetime supervision; its config and
state formats are refused, never converted.
"""

from __future__ import annotations

import ctypes
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from collections.abc import Callable, Generator
from pathlib import Path

import pytest


AU_ROOT = Path(__file__).resolve().parents[2]

from tests.rust_binary import rust_agentctl_with_fake as _binaries


FAKE_HERDR = r"""
import json, os, sys
from pathlib import Path

control = Path(os.environ["FAKE_HERDR_DIR"])
args = sys.argv[1:]

# A call that starts with one of these words never answers, as a Herdr call stuck behind a busy
# pane does; it leaves a marker naming itself first (#870 agentctl-service-slow-stop).
for hang in filter(None, os.environ.get("FAKE_HERDR_HANG", "").split(",")):
    if args[:len(hang.split())] == hang.split():
        (control / "hung").write_text(" ".join(args))
        import time
        time.sleep(600)

def pane():
    harness = control / "harness.pid"
    agent = None
    session = None
    if harness.exists():
        agent = "codex"
        session = {"agent": "codex", "value": "sess-1"}
    return {
        "pane_id": "w7:p3",
        "workspace_id": "w7",
        "cwd": os.environ["FAKE_PANE_CWD"],
        "agent": agent,
        "agent_status": os.environ.get("FAKE_AGENT_STATUS", "working") if agent else "unknown",
        "agent_session": session,
        "terminal_id": "term-1",
        "tab_id": "w7:t1",
    }

if args[:2] == ["pane", "read"] and "FAKE_SCREEN" in os.environ:
    print(os.environ["FAKE_SCREEN"], end="")
elif args[:2] == ["pane", "get"]:
    print(json.dumps({"result": {"pane": pane()}}))
elif args[:2] == ["workspace", "get"]:
    print(json.dumps({"result": {"workspace": {"workspace_id": "w7", "label": "project-space"}}}))
elif args[:2] == ["pane", "list"]:
    print(json.dumps({"result": {"panes": [
        {"pane_id": "w7:p3", "tab_id": "w7:t1", "workspace_id": "w7"}
    ]}}))
elif args[:2] == ["pane", "process-info"]:
    shell_pid = int((control / "shell.pid").read_text())
    harness = control / "harness.pid"
    if harness.exists():
        leader = int(harness.read_text())
        processes = [{"pid": leader}]
        foreground = leader
    else:
        processes = [{"pid": shell_pid}]
        foreground = shell_pid
    print(json.dumps({"result": {"process_info": {
        "pane_id": "w7:p3",
        "shell_pid": shell_pid,
        "foreground_process_group_id": foreground,
        "foreground_processes": processes,
    }}}))
else:
    print(json.dumps({"result": {}}))
"""


class Fixture:
    def __init__(self, tmp_path: Path) -> None:
        self.agentctl, self.plugin_binary = _binaries()
        self.root = tmp_path
        self.control = tmp_path / "herdr-control"
        self.control.mkdir()
        self.bin_dir = tmp_path / "bin"
        self.bin_dir.mkdir()
        self.registry = tmp_path / "registry"
        self.registry.mkdir()
        self.state = tmp_path / "chat-state"
        self.home = tmp_path / "agentctl-home"
        plugin_dir = self.home / "plugins" / "fakechat"
        plugin_dir.mkdir(parents=True)
        for directory in (self.home, self.home / "plugins", plugin_dir):
            directory.chmod(0o700)
        backend = plugin_dir / "backend"
        shutil.copy(self.plugin_binary, backend)
        backend.chmod(0o700)
        manifest = {
            "schema": "agentctl-plugin-manifest/v1alpha1",
            "name": "fakechat",
            "capability": "chat-subscription.fake",
            "executable": "backend",
            "protocol": {"name": "agentctl-chat-subscription", "min": 1, "max": 1},
            "implementation": "implemented",
        }
        manifest_path = plugin_dir / "manifest.json"
        manifest_path.write_text(json.dumps(manifest))
        manifest_path.chmod(0o600)
        herdr = self.bin_dir / "herdr"
        herdr.write_text("#!/usr/bin/env python3\n" + FAKE_HERDR)
        herdr.chmod(0o755)
        self.herdr = herdr
        harness = self.bin_dir / "codex"
        harness.write_text(
            "#!/bin/bash\n"
            'echo $$ > "$FAKE_HERDR_DIR/harness.pid"\n'
            'if [ -n "$FAKE_HARNESS_EXIT" ]; then sleep 1; exit "$FAKE_HARNESS_EXIT"; fi\n'
            "exec sleep 600\n"
        )
        harness.chmod(0o755)
        self.config = tmp_path / "chat.json"
        self.config.write_text(
            json.dumps(
                {
                    "subscription_plugin": "fakechat",
                    "channel_ids": ["spaces/example"],
                    "allowed_senders": ["users/owner"],
                    "agent_name": "coordinator",
                    "agent_label": "test-coordinator",
                    "outbound_enabled": False,
                    "ack_reaction": None,
                }
            )
        )
        self.config.chmod(0o600)
        self.shell = subprocess.Popen(
            ["/bin/bash", "-c", "while true; do sleep 5; done"],
            preexec_fn=os.setsid,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        (self.control / "shell.pid").write_text(str(self.shell.pid))

    def env(self) -> dict[str, str]:
        env = dict(os.environ)
        env.update(
            {
                "HERDR_ENV": "1",
                "HERDR_PANE_ID": "w7:p3",
                "HERDR_WORKSPACE_ID": "w7",
                "FAKE_HERDR_DIR": str(self.control),
                "FAKE_PANE_CWD": str(self.root),
                "AGENTCTL_HOME": str(self.home),
                # `chat run` registers the bridge in the per-user index (#707
                # chat-prompt-anatomy); keep that inside the fixture.
                "AGENTCTL_CHAT_BRIDGE_INDEX": str(self.root / "chat-bridges"),
                # One bridge per channel is host-wide; concurrent test runs share channel IDs.
                "AGENTCTL_CHAT_CHANNEL_LOCK_DIR": str(self.root / "channel-locks"),
                "PATH": f"{self.bin_dir}:{env['PATH']}",
            }
        )
        env.pop("FAKE_HARNESS_EXIT", None)
        return env

    def launch(
        self,
        *,
        extra_env: dict[str, str] | None = None,
        config: Path | None = None,
        args: list[str] | None = None,
    ) -> subprocess.Popen[str]:
        env = self.env()
        if extra_env:
            env.update(extra_env)
        parent_pid = os.getpid()

        def die_with_parent() -> None:
            # The launcher must not outlive this test process: if the outer
            # pytest is killed (SIGKILL included), the kernel delivers
            # SIGKILL to the launcher, whose own launch-gate children carry
            # the same parent-death lifeline onward to the bridge and the
            # harness (#857). Without this, a killed gate step left a
            # `chat run` reparented to init for 34 hours.
            libc = ctypes.CDLL(None, use_errno=True)
            if os.getppid() != parent_pid:
                os.kill(os.getpid(), signal.SIGKILL)
            if libc.prctl(1, int(signal.SIGKILL), 0, 0, 0) != 0:
                os.kill(os.getpid(), signal.SIGKILL)
            if os.getppid() != parent_pid:
                os.kill(os.getpid(), signal.SIGKILL)

        return subprocess.Popen(
            [
                str(self.agentctl),
                "--registry", str(self.registry),
                "--herdr-bin", str(self.herdr),
                "chat", "launch",
                "--bridge-state", str(self.state),
                "--config", str(config or self.config),
                *(args or []),
            ],
            cwd=self.root,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
            preexec_fn=die_with_parent,
        )

    def processes_with(self, needle: str) -> list[int]:
        found: list[int] = []
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                # Compare as bytes: any host process may carry non-UTF-8 argv.
                cmdline = (entry / "cmdline").read_bytes().replace(b"\0", b" ")
            except OSError:
                continue
            if needle.encode() in cmdline and b"chat_launch_rust" not in cmdline:
                found.append(int(entry.name))
        return found

    def bridge_processes(self) -> list[int]:
        # The exec'd bridge only: its gate names the launch gate, and
        # bridge helpers do not carry the `chat run` token sequence.
        found: list[int] = []
        needle = f" chat run --bridge-state {self.state} "
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                # Compare as bytes: any host process may carry non-UTF-8 argv.
                cmdline = (entry / "cmdline").read_bytes().replace(b"\0", b" ")
            except OSError:
                continue
            if needle.encode() in cmdline and b"--chat-launch-gate" not in cmdline:
                found.append(int(entry.name))
        return found

    def close(self) -> None:
        self.shell.kill()
        self.shell.wait()
        # Finally-net (#857): nothing the fixture started may outlive the
        # test, even when a test failed before its launcher exited. The
        # launcher normally stops its children itself; kill any straggler
        # that still carries this fixture's state path.
        for pid in self.processes_with(str(self.state)):
            try:
                os.kill(pid, signal.SIGKILL)
            except OSError:
                pass


@pytest.fixture()
def fixture(tmp_path: Path) -> Generator[Fixture, None, None]:
    made = Fixture(tmp_path)
    yield made
    made.close()


def _wait_for(
    predicate: Callable[[], bool], timeout: float = 30.0, interval: float = 0.1
) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return False


def test_launch_refuses_python_format_config(fixture: Fixture, tmp_path: Path) -> None:
    legacy = tmp_path / "legacy.json"
    legacy.write_text(json.dumps({"space": "spaces/test", "allowed_senders": ["users/o"]}))
    legacy.chmod(0o600)
    process = fixture.launch(config=legacy)
    stdout, stderr = process.communicate(timeout=120)
    assert process.returncode == 1
    assert "Python-format chat config" in stderr
    assert "chat init" in stderr
    assert not fixture.state.exists()
    assert not (fixture.control / "harness.pid").exists()
    assert stdout == ""


def test_launch_refuses_python_polling_state(fixture: Fixture) -> None:
    fixture.state.mkdir()
    (fixture.state / "bridge.json").write_text(
        json.dumps({"version": 1, "config": {"space": "spaces/test", "target": {}}})
    )
    process = fixture.launch()
    _stdout, stderr = process.communicate(timeout=120)
    assert process.returncode == 1
    assert "Python polling bridge state" in stderr
    assert "chat init" in stderr
    assert not (fixture.control / "harness.pid").exists()


def test_launch_refuses_outside_herdr_pane(fixture: Fixture) -> None:
    process = fixture.launch(extra_env={"HERDR_ENV": "0"})
    _stdout, stderr = process.communicate(timeout=120)
    assert process.returncode == 1
    assert "must run inside the Herdr shell pane" in stderr


def test_launch_returns_coordinator_exit_code(fixture: Fixture) -> None:
    process = fixture.launch(extra_env={"FAKE_HARNESS_EXIT": "7"})
    _stdout, _stderr = process.communicate(timeout=300)
    assert process.returncode == 7
    assert (fixture.registry / "agents" / "coordinator" / "agent.json").exists()
    assert (fixture.state / "bridge.json").exists()
    assert (fixture.state / "bridge.log").exists()
    assert _wait_for(lambda: not fixture.processes_with(str(fixture.state)), 120)


def test_launch_sigterm_stops_both_children(fixture: Fixture) -> None:
    process = fixture.launch()
    assert _wait_for(lambda: (fixture.control / "harness.pid").exists(), 300)
    harness_pid = int((fixture.control / "harness.pid").read_text())
    assert _wait_for(lambda: bool(fixture.processes_with(str(fixture.state))), 300)
    process.send_signal(signal.SIGTERM)
    _stdout, _stderr = process.communicate(timeout=120)
    assert process.returncode == 0
    assert _wait_for(lambda: not Path(f"/proc/{harness_pid}").exists(), 120)
    assert _wait_for(lambda: not fixture.processes_with(str(fixture.state)), 120)


def test_launch_sigterm_exits_within_a_few_seconds(fixture: Fixture) -> None:
    # Regression: admission work (pane flock / late-paste polling) must
    # never keep `chat run` alive until systemd's stop timeout. Exit is
    # the condition; the generous communicate bound only reports a hang
    # (owner rule 346: no wall-clock verdict).
    process = fixture.launch()
    assert _wait_for(lambda: (fixture.control / "harness.pid").exists(), 300)
    assert _wait_for(lambda: bool(fixture.processes_with(str(fixture.state))), 300)
    process.send_signal(signal.SIGTERM)
    _stdout, _stderr = process.communicate(timeout=120)
    assert process.returncode == 0


def _process_gone(pid: int) -> bool:
    try:
        state = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
    except OSError:
        return True
    return state == "Z"


def test_run_sigterm_with_a_delivery_in_flight_exits_within_five_seconds(
    fixture: Fixture, tmp_path: Path
) -> None:
    # #870 agentctl-service-slow-stop: `chat run` exits within 5 s of SIGTERM while it is
    # delivering an owner request whose typing never finishes, and the request is still waiting
    # for delivery afterwards, never recorded as delivered.
    config = tmp_path / "conformance.json"
    document = json.loads(fixture.config.read_text())
    document["channel_ids"] = ["channels/conformance"]
    # The fake plugin's conformance script serves its one owner message only to this exact
    # backend configuration.
    document["backend_configuration"] = {
        "schema": "fixture.config.v1",
        "data": {"subscription": "subscriptions/conformance"},
    }
    document["subscription_environment"] = ["CHAT_SUBSCRIPTION_FAKE_SCRIPT"]
    config.write_text(json.dumps(document))
    config.chmod(0o600)
    process = fixture.launch(
        config=config,
        extra_env={
            "CHAT_SUBSCRIPTION_FAKE_SCRIPT": "checkpoint-first",
            "FAKE_AGENT_STATUS": "idle",
            "FAKE_SCREEN": "• Ready for the next task.\n\n› \n",
            "FAKE_HERDR_HANG": "pane send-text,pane send-keys",
        },
    )
    try:
        assert _wait_for(lambda: bool(fixture.bridge_processes()), 300)
        bridge = fixture.bridge_processes()[0]
        log = fixture.state / "bridge.log"

        def admitted() -> list[Path]:
            return list(fixture.state.glob("requests/*.json"))

        assert _wait_for(lambda: bool(admitted()), 300), (
            "no request was admitted: "
            + (log.read_text()[-3000:] if log.exists() else "no bridge log")
        )
        # The owner loop starts delivering the request at once, and each attempt waits for the
        # pane's composer, or hangs in Herdr's typing call, for longer than this.
        time.sleep(2)
        os.kill(bridge, signal.SIGTERM)
        # Exit is the condition; the generous bound only reports a hang
        # (owner rule 346: no wall-clock verdict).
        assert _wait_for(lambda: _process_gone(bridge), 120)
        status = subprocess.run(
            [
                str(fixture.agentctl),
                "--registry", str(fixture.registry),
                "--herdr-bin", str(fixture.herdr),
                "chat", "status", "--bridge-state", str(fixture.state),
            ],
            env=fixture.env(),
            capture_output=True,
            text=True,
            timeout=60,
        )
        assert "first message" not in status.stdout or "delivered" not in status.stdout, status.stdout
        requests = admitted()
        assert requests, list(fixture.state.iterdir())
        for path in requests:
            record = json.loads(path.read_text())
            assert record.get("delivered_at_millis") is None, record
    finally:
        process.send_signal(signal.SIGTERM)
        process.communicate(timeout=120)


def test_run_sigterm_with_a_plugin_that_ignores_close_exits_within_five_seconds(
    fixture: Fixture, tmp_path: Path
) -> None:
    # #870 agentctl-service-slow-stop: a provider plugin that never answers Close makes the stop
    # report "plugin required forced shutdown"; `chat run` still exits within 5 s of SIGTERM.
    config = tmp_path / "stalling.json"
    document = json.loads(fixture.config.read_text())
    document["subscription_environment"] = ["CHAT_SUBSCRIPTION_FAKE_STALL"]
    config.write_text(json.dumps(document))
    config.chmod(0o600)
    process = fixture.launch(config=config, extra_env={"CHAT_SUBSCRIPTION_FAKE_STALL": "commit"})
    try:
        assert _wait_for(lambda: bool(fixture.bridge_processes()), 300)
        bridge = fixture.bridge_processes()[0]
        log = fixture.state / "bridge.log"
        assert _wait_for(
            lambda: log.exists() and "subscribed without a saved cursor" in log.read_text(), 60
        ), (log.read_text()[-3000:] if log.exists() else "no bridge log")
        time.sleep(1)
        started = time.monotonic()
        os.kill(bridge, signal.SIGTERM)
        assert _wait_for(lambda: _process_gone(bridge), 120)
        took = time.monotonic() - started
        assert took < 5, f"chat run took {took:.1f}s to stop: {log.read_text()[-2000:]}"
    finally:
        process.send_signal(signal.SIGTERM)
        process.communicate(timeout=120)


def _children(pid: int) -> list[int]:
    found: list[int] = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
        except (OSError, IndexError):
            continue
        if fields[1] == str(pid):
            found.append(int(entry.name))
    return found


@pytest.mark.parametrize("moment", ["at-exec", "plugin-handshake"])
def test_run_sigterm_during_startup_exits_within_five_seconds(
    fixture: Fixture, tmp_path: Path, moment: str
) -> None:
    # #870 agentctl-service-slow-stop: a stop that lands while `chat run` is still starting,
    # right after it was exec'd or while its provider plugin has not finished its handshake,
    # ends within 5 s, as a stop of a running bridge does.
    if moment == "plugin-handshake":
        config = tmp_path / "startup.json"
        document = json.loads(fixture.config.read_text())
        document["subscription_environment"] = ["CHAT_SUBSCRIPTION_FAKE_STALL"]
        config.write_text(json.dumps(document))
        config.chmod(0o600)
        process = fixture.launch(config=config, extra_env={"CHAT_SUBSCRIPTION_FAKE_STALL": "hello"})
    else:
        process = fixture.launch()
    try:
        assert _wait_for(lambda: bool(fixture.bridge_processes()), 300, interval=0.01)
        bridge = fixture.bridge_processes()[0]
        log = fixture.state / "bridge.log"
        if moment == "plugin-handshake":
            # The plugin is spawned and waits for ever before its hello.
            assert _wait_for(lambda: bool(_children(bridge)), 300, interval=0.01), (
                log.read_text()[-3000:] if log.exists() else "no bridge log"
            )
            time.sleep(0.5)
        os.kill(bridge, signal.SIGTERM)
        # Exit is the condition; the generous bound only reports a hang.
        assert _wait_for(lambda: _process_gone(bridge), 120)
        # It stopped by itself: the termination guard, which abandons a stop after 4 s, did
        # not have to end it.
        text = log.read_text() if log.exists() else ""
        assert "did not stop within" not in text, text[-2000:]
    finally:
        process.send_signal(signal.SIGTERM)
        process.communicate(timeout=120)


def test_launch_agent_label_override_is_persisted(fixture: Fixture) -> None:
    process = fixture.launch(
        extra_env={"FAKE_HARNESS_EXIT": "0"}, args=["--agent-label", "custom-label"]
    )
    _stdout, _stderr = process.communicate(timeout=300)
    assert process.returncode == 0
    document = json.loads((fixture.state / "bridge.json").read_text())
    assert document["config"]["agent_label"] == "custom-label"


def test_launch_relaunch_refuses_registered_name_before_spawning(
    fixture: Fixture,
) -> None:
    first = fixture.launch(extra_env={"FAKE_HARNESS_EXIT": "0"})
    _stdout, _stderr = first.communicate(timeout=300)
    assert first.returncode == 0
    # The pane is a shell pane again (the harness exited), but the
    # registry name stays bound: launch refuses before spawning.
    (fixture.control / "harness.pid").unlink()
    second = fixture.launch(extra_env={"FAKE_HARNESS_EXIT": "0"})
    _stdout, stderr = second.communicate(timeout=300)
    assert second.returncode == 1
    assert "already registered" in stderr
    assert not (fixture.control / "harness.pid").exists()


def test_launch_bridge_killed_first_is_an_error_and_stops_coordinator(
    fixture: Fixture,
) -> None:
    process = fixture.launch()
    assert _wait_for(lambda: (fixture.control / "harness.pid").exists(), 300)
    harness_pid = int((fixture.control / "harness.pid").read_text())
    bridges: list[int] = []

    def find_bridge() -> bool:
        bridges.clear()
        bridges.extend(
            pid for pid in fixture.bridge_processes() if pid != process.pid
        )
        return bool(bridges)

    assert _wait_for(find_bridge, 60)
    os.kill(bridges[0], signal.SIGKILL)
    _stdout, stderr = process.communicate(timeout=300)
    assert process.returncode == 1
    assert "chat bridge exited" in stderr
    assert _wait_for(lambda: not Path(f"/proc/{harness_pid}").exists(), 120)


_OUTER_KILL_HELPER = '''
import importlib.util
import json
import os
import time
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "chat_launch_rust_module", os.environ["CHAT_LAUNCH_MODULE"]
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def test_outer_fixture_sleeps(tmp_path):
    made = module.Fixture(tmp_path)
    process = made.launch()
    assert module._wait_for(lambda: (made.control / "harness.pid").exists(), 300)
    bridges = []

    def find_bridge():
        bridges.clear()
        bridges.extend(
            pid for pid in made.bridge_processes() if pid != process.pid
        )
        return bool(bridges)

    assert module._wait_for(find_bridge, 60)
    Path(os.environ["CHAT_LAUNCH_MARKER"]).write_text(
        json.dumps(
            {
                "state": str(made.state),
                "launcher_pid": process.pid,
                "bridge_pid": bridges[0],
                "harness_pid": int((made.control / "harness.pid").read_text()),
            }
        )
    )
    time.sleep(600)
'''


def test_killing_outer_pytest_leaves_no_chat_run(tmp_path: Path) -> None:
    """SIGKILL of the outer pytest must take the whole fixture tree down (#857)."""
    helper = tmp_path / "test_outer_kill_helper.py"
    helper.write_text(_OUTER_KILL_HELPER)
    marker = tmp_path / "marker.json"
    env = dict(os.environ)
    env["CHAT_LAUNCH_MODULE"] = str(Path(__file__).resolve())
    env["CHAT_LAUNCH_MARKER"] = str(marker)
    env["PYTHONPATH"] = str(AU_ROOT / "py") + os.pathsep + env.get("PYTHONPATH", "")
    outer = subprocess.Popen(
        # The inner run's temporary tree goes under this test's, so every process it starts
        # names this test's tree and `survivors` below can find it.
        [sys.executable, "-m", "pytest", str(helper), "-x", "-q", "-p", "no:cacheprovider",
         "--basetemp", str(tmp_path / "inner")],
        cwd=tmp_path,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
    )
    record: dict[str, object] = {}

    def survivors() -> list[int]:
        # Every process still naming this fixture's tree: the bridge, the harness, the launcher,
        # and anything they started, such as the subscription plugin's supervisor, which carries
        # the bridge's command line (#1061 chat-launch-outer-kill-leak: one lived for two days).
        found = []
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit() or int(entry.name) == os.getpid():
                continue
            try:
                cmdline = (entry / "cmdline").read_bytes()
                state = next(
                    (line for line in (entry / "status").read_text().splitlines()
                     if line.startswith("State:")),
                    "",
                )
            except OSError:
                continue
            if str(tmp_path).encode() in cmdline and "zombie" not in state:
                found.append(int(entry.name))
        return found

    try:
        assert _wait_for(lambda: marker.exists(), 300), "outer pytest never launched its fixture"
        record = json.loads(marker.read_text())
        outer.kill()  # SIGKILL to the pytest process only, as a killed gate step does.
        outer.wait(timeout=120)
        assert _wait_for(lambda: not survivors(), 300), (
            f"fixture processes survived the outer kill: {survivors()} ({record})"
        )
    finally:
        if outer.poll() is None:
            outer.kill()
            outer.wait(timeout=120)
        # Whatever the verdict, nothing this test started outlives it. Each pid is checked to
        # still name this test's temporary tree just before it is killed, so a reused pid is
        # never hit.
        for pid in survivors():
            try:
                if str(tmp_path).encode() in Path(f"/proc/{pid}/cmdline").read_bytes():
                    os.kill(pid, signal.SIGKILL)
            except OSError:
                pass
