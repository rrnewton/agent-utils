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

import json
import os
import shutil
import signal
import subprocess
import time
from collections.abc import Callable, Generator
from pathlib import Path

import pytest


AU_ROOT = Path(__file__).resolve().parents[2]
TARGET = "x86_64-unknown-linux-gnu"


def _binaries() -> tuple[Path, Path]:
    env = dict(os.environ)
    env.update(
        {
            "CARGO_PROFILE_DEV_DEBUG": "line-tables-only",
            "CARGO_PROFILE_TEST_DEBUG": "line-tables-only",
            "CARGO_INCREMENTAL": "0",
        }
    )
    subprocess.run(
        [
            "cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
            "--target", TARGET, "-p", "agentctl", "-p", "chat-subscription-fake",
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=True, timeout=1200, env=env,
    )
    release = AU_ROOT / "rs" / "target" / TARGET / "release"
    return release / "agentctl", release / "chat-subscription-fake-plugin"


FAKE_HERDR = r"""
import json, os, sys
from pathlib import Path

control = Path(os.environ["FAKE_HERDR_DIR"])
args = sys.argv[1:]

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
        "agent_status": "working" if agent else "unknown",
        "agent_session": session,
        "terminal_id": "term-1",
        "tab_id": "w7:t1",
    }

if args[:2] == ["pane", "get"]:
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
        )

    def processes_with(self, needle: str) -> list[int]:
        found: list[int] = []
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                cmdline = (entry / "cmdline").read_bytes().replace(b"\0", b" ").decode()
            except OSError:
                continue
            if needle in cmdline and "chat_launch_rust" not in cmdline:
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
                cmdline = (entry / "cmdline").read_bytes().replace(b"\0", b" ").decode()
            except OSError:
                continue
            if needle in cmdline and "--chat-launch-gate" not in cmdline:
                found.append(int(entry.name))
        return found

    def close(self) -> None:
        self.shell.kill()
        self.shell.wait()


@pytest.fixture()
def fixture(tmp_path: Path) -> Generator[Fixture, None, None]:
    made = Fixture(tmp_path)
    yield made
    made.close()


def _wait_for(predicate: Callable[[], bool], timeout: float = 30.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
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
    assert _wait_for(lambda: not fixture.processes_with(str(fixture.state)), 15)


def test_launch_sigterm_stops_both_children(fixture: Fixture) -> None:
    process = fixture.launch()
    assert _wait_for(lambda: (fixture.control / "harness.pid").exists(), 60)
    harness_pid = int((fixture.control / "harness.pid").read_text())
    assert _wait_for(lambda: bool(fixture.processes_with(str(fixture.state))), 60)
    process.send_signal(signal.SIGTERM)
    _stdout, _stderr = process.communicate(timeout=120)
    assert process.returncode == 0
    assert _wait_for(lambda: not Path(f"/proc/{harness_pid}").exists(), 15)
    assert _wait_for(lambda: not fixture.processes_with(str(fixture.state)), 15)


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
    assert _wait_for(lambda: (fixture.control / "harness.pid").exists(), 60)
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
    assert _wait_for(lambda: not Path(f"/proc/{harness_pid}").exists(), 15)
