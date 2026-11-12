"""Guarded kill-pid (#1420): fixture /proc and systemctl, fake killer only.

No test in this file signals a real process: the process table and the unit
list are fixtures, and the killer records what it would have sent.
"""
from __future__ import annotations

import json
import signal
from pathlib import Path

import pytest

from agentctl.killpid import (
    KillRefused, ProcessInfo, ProcView, UnitInfo, guarded_kill,
)


class FakeProc:
    """A fixture /proc: one process table, optionally changing on re-read."""

    def __init__(self, processes: dict[int, ProcessInfo],
                 changed: ProcessInfo | None = None) -> None:
        self._processes = processes
        self._changed = changed
        self.reads = 0

    def process(self, pid: int) -> ProcessInfo | None:
        self.reads += 1
        if self._changed is not None and self.reads > 1:
            return self._changed
        return self._processes.get(pid)


class FakeUnits:
    """A fixture systemctl: a fixed unit list."""

    def __init__(self, units: tuple[UnitInfo, ...] = ()) -> None:
        self._units = units

    def units(self) -> tuple[UnitInfo, ...]:
        return self._units


class FakeKiller:
    """Records signals instead of sending them; re-verifies like the real one."""

    def __init__(self) -> None:
        self.sent: list[tuple[int, int]] = []

    def send(self, pid: int, signum: int, view: ProcView, expected: ProcessInfo) -> None:
        current = view.process(pid)
        if current is None or current.starttime != expected.starttime \
                or current.cmdline != expected.cmdline:
            raise KillRefused(
                "changed-before-signal",
                f"pid {pid} changed between the check and the signal; not signalling",
            )
        self.sent.append((pid, signum))


def proc(pid: int, argv: tuple[str, ...], starttime: int, cwd: str | None) -> ProcessInfo:
    return ProcessInfo(pid=pid, argv=argv, starttime=starttime, cwd=cwd,
                       cgroup="", comm=argv[0] if argv else "")


@pytest.fixture()
def registry(tmp_path: Path) -> Path:
    path = tmp_path / "team" / ".agentctl"
    path.mkdir(parents=True)
    return path


def kill(registry: Path, view: FakeProc, units: FakeUnits, killer: FakeKiller,
         pid: int, expect: str, start: int) -> object:
    return guarded_kill(
        pid=pid, expect_cmdline=expect, expect_start=start, signal_name="TERM",
        registry=registry, proc=view, unit_view=units, killer=killer,
        self_pid=999_999, now=1_700_000_000.0,
    )


def test_owned_child_is_signalled_and_logged(registry: Path) -> None:
    team_root = str(registry.parent)
    view = FakeProc({4242: proc(4242, ("sleep", "60"), 111, team_root)})
    killer = FakeKiller()
    report = kill(registry, view, FakeUnits(), killer, 4242, "sleep 60", 111)
    assert killer.sent == [(4242, int(signal.SIGTERM))]
    log = (registry / "kill-pid.jsonl").read_text(encoding="utf-8")
    record = json.loads(log)
    assert record["schema"] == "agentctl-kill-pid/v1"
    assert record["pid"] == 4242
    assert record["signal"] == "TERM"
    assert record["starttime"] == 111
    assert getattr(report, "pid") == 4242


def test_owned_unit_main_pid_is_signalled(registry: Path) -> None:
    (registry / "restore.json").write_text(
        json.dumps({"units": ["chat-bridge.service"]}), encoding="utf-8")
    view = FakeProc({4242: proc(4242, ("agentctl", "chat", "run"), 111, "/elsewhere")})
    units = FakeUnits((UnitInfo(name="chat-bridge.service", main_pid=4242),))
    killer = FakeKiller()
    kill(registry, view, units, killer, 4242, "agentctl chat run", 111)
    assert killer.sent == [(4242, int(signal.SIGTERM))]


@pytest.mark.parametrize(("info", "units", "expect", "start", "reason"), [
    (proc(4242, ("/usr/lib/systemd/systemd", "--user"), 111, "/team"),
     (), "systemd --user", 111, "user-manager"),
    (proc(4242, ("/usr/lib/systemd/systemd", "--user"), 111, "/team"),
     (UnitInfo(name="user@1000.service", main_pid=4242),),
     "systemd --user", 111, "user-manager"),
    (proc(4242, ("herdr", "server"), 111, "/team"),
     (), "herdr server", 111, "herdr-server"),
    (proc(4242, ("sleep", "60"), 111, "/team"),
     (UnitInfo(name="other.service", main_pid=4242),),
     "sleep 60", 111, "foreign-unit-main-pid"),
    (proc(4242, ("sleep", "60"), 111, "/elsewhere/team/.agentctl/agents/x"),
     (), "sleep 60", 111, "other-team"),
    (proc(4242, ("sleep", "60"), 111, "/tmp"),
     (), "sleep 60", 111, "not-owned"),
    (proc(4242, ("sleep", "60"), 111, "/team"),
     (), "sleep 60", 999, "starttime-mismatch"),
    (proc(4242, ("sleep", "60"), 111, "/team"),
     (), "other cmdline", 111, "cmdline-mismatch"),
])
def test_refusals(registry: Path, info: ProcessInfo, units: tuple[UnitInfo, ...],
                  expect: str, start: int, reason: str) -> None:
    # The team root of the fixture registry is tmp_path/team; point owned cases there.
    if info.cwd == "/team":
        info = proc(info.pid, info.argv, info.starttime, str(registry.parent))
    view = FakeProc({info.pid: info})
    killer = FakeKiller()
    with pytest.raises(KillRefused) as caught:
        kill(registry, view, FakeUnits(units), killer, info.pid, expect, start)
    assert caught.value.reason == reason
    assert killer.sent == []
    assert not (registry / "kill-pid.jsonl").exists()


def test_missing_process_and_pid_one_are_refused(registry: Path) -> None:
    killer = FakeKiller()
    with pytest.raises(KillRefused) as caught:
        kill(registry, FakeProc({}), FakeUnits(), killer, 4242, "sleep", 1)
    assert caught.value.reason == "no-such-process"
    view = FakeProc({1: proc(1, ("sleep", "60"), 111, str(registry.parent))})
    with pytest.raises(KillRefused) as caught:
        kill(registry, view, FakeUnits(), killer, 1, "sleep 60", 111)
    assert caught.value.reason == "protected-pid"
    assert killer.sent == []


def test_changed_before_signal_is_refused(registry: Path) -> None:
    team_root = str(registry.parent)
    before = proc(4242, ("sleep", "60"), 111, team_root)
    after = proc(4242, ("sleep", "60"), 222, team_root)
    view = FakeProc({4242: before}, changed=after)
    killer = FakeKiller()
    with pytest.raises(KillRefused) as caught:
        kill(registry, view, FakeUnits(), killer, 4242, "sleep 60", 111)
    assert caught.value.reason == "changed-before-signal"
    assert killer.sent == []


def test_cli_parses_kill_pid() -> None:
    from agentctl.cli import parser
    args = parser().parse_args(
        ["kill-pid", "4242", "--expect-cmdline", "sleep 60", "--expect-start", "111"])
    assert args.command == "kill-pid"
    assert args.pid == 4242
    assert args.expect_cmdline == "sleep 60"
    assert args.expect_start == 111
    assert args.signal_name == "TERM"
