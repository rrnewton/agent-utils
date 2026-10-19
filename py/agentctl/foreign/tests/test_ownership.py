"""Generation-safe process ownership: pinning, tree cleanup, and their refusals.

Every process these tests signal is one they started (or a descendant of one).
PID and process-group reuse are simulated by recording a stale start tick or
boot identity for a live child and asserting that the child survives.
"""
from __future__ import annotations

import ast
import contextlib
import json
import os
import secrets
import signal
import subprocess
import sys
import time
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Protocol

import pytest

from agentctl.foreign import lib, ownership
from agentctl.procstat import ProcessStat

PY_ROOT = Path(ownership.__file__).resolve().parents[2]
SLEEP = (sys.executable, "-c", "import time; time.sleep(30)")


# --------------------------------------------------------------------------- #
# Helpers
# --------------------------------------------------------------------------- #


def _stat(pid: int) -> ProcessStat | None:
    try:
        return ownership.read_stat(pid)
    except lib.AgentOperationError:
        return None


def _running(pid: int, starttime: int | None = None) -> bool:
    current = _stat(pid)
    return (current is not None and current.state not in ("Z", "X", "x")
            and (starttime is None or current.starttime == starttime))


def _wait_until(predicate: Callable[[], bool], timeout: float = 10.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.02)
    return predicate()


def _identity(pid: int) -> ownership.ProcessIdentity:
    current = ownership.read_stat(pid)
    assert current is not None
    return ownership.ProcessIdentity(pid, str(current.starttime), ownership.current_boot_id())


class _Descendants:
    """Processes this test started, directly or through its children.

    Teardown pins each by start tick and kills it only through a pidfd, so a
    number the kernel has meanwhile reused is never touched.
    """

    def __init__(self) -> None:
        self.processes: dict[int, int] = {}

    def add(self, pid: int) -> int:
        current = _stat(pid)
        if current is not None:
            self.processes[pid] = current.starttime
        return pid

    def alive(self, pid: int) -> bool:
        return _running(pid, self.processes.get(pid))

    def cleanup(self) -> None:
        for pid, starttime in self.processes.items():
            result = ownership.pin(ownership.ProcessIdentity(pid, str(starttime), ownership.current_boot_id()))
            if result.process is not None:
                with result.process as process:
                    process.send(signal.SIGKILL)
                    process.wait(5)


@pytest.fixture()
def descendants() -> Iterator[_Descendants]:
    tracked = _Descendants()
    try:
        yield tracked
    finally:
        tracked.cleanup()


@pytest.fixture()
def child(descendants: _Descendants) -> Iterator[subprocess.Popen[bytes]]:
    process = subprocess.Popen(SLEEP, start_new_session=True)
    descendants.add(process.pid)
    try:
        yield process
    finally:
        with contextlib.suppress(OSError):
            process.kill()
        process.wait(timeout=5)


_TREE = r'''
import json, os, subprocess, sys, time
out, mode = sys.argv[1], sys.argv[2]
clean = {"PATH": os.environ.get("PATH", "")}
sleep = [sys.executable, "-c", "import time; time.sleep(30)"]
pids = {}
pids["session_child"] = subprocess.Popen(sleep).pid
pids["escapee_with_generation"] = subprocess.Popen(sleep, start_new_session=True).pid
pids["session_without_generation"] = subprocess.Popen(sleep, env=clean).pid
read, write = os.pipe()
middle = os.fork()
if middle == 0:
    os.setsid()
    grandchild = os.fork()
    if grandchild == 0:
        os.execve(sys.executable, sleep, clean)
    os.write(write, str(grandchild).encode())
    os._exit(0)
os.waitpid(middle, 0)
pids["double_fork_without_generation"] = int(os.read(read, 32))
pids["leader"] = os.getpid()
temporary = out + ".tmp"
with open(temporary, "w") as stream:
    json.dump(pids, stream)
os.replace(temporary, out)
if mode == "exit":
    sys.exit(0)
time.sleep(30)
'''

_DRIVER = r'''
import json, os, sys
sys.path.insert(0, sys.argv[1])
from pathlib import Path
from agentctl.foreign import ownership
argv, record_path = json.loads(sys.argv[2]), Path(sys.argv[3])
if os.environ.get("TEST_NO_PIDFD") == "1":
    ownership.pidfds_supported = lambda: False
ownership.set_child_subreaper()
def record(value):
    temporary = record_path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value.document("recorded")))
    os.replace(temporary, record_path)
stat = ownership.read_stat(os.getpid())
runner = ownership.ProcessIdentity(os.getpid(), str(stat.starttime), ownership.current_boot_id())
proc = ownership.spawn_owned_harness(argv, cwd=str(record_path.parent), stdin=False, runner=runner, record=record)
print("spawned", proc.pid, flush=True)
command = sys.stdin.readline().strip()
if command == "kill_tree":
    report = proc.kill_tree()
    proc.wait()
elif command == "wait":
    proc.wait()
    report = proc.cleanup
else:
    raise SystemExit(f"unknown command {command!r}")
proc.finish()
print(json.dumps({"returncode": proc.returncode,
                  "complete": report is None or report.complete,
                  "killed": 0 if report is None else report.killed,
                  "describe": "" if report is None else report.describe()}), flush=True)
sys.stdin.readline()
'''


class _Runner:
    """A stand-in runner process owning one harness tree."""

    def __init__(self, tmp_path: Path, descendants: _Descendants, mode: str,
                 *, environment: dict[str, str] | None = None) -> None:
        self.tmp_path = tmp_path
        self.descendants = descendants
        tree = tmp_path / "tree.py"
        tree.write_text(_TREE)
        driver = tmp_path / "driver.py"
        driver.write_text(_DRIVER)
        self.pids_path = tmp_path / "pids.json"
        self.record_path = tmp_path / "record.json"
        env = dict(os.environ)
        env.pop(ownership.CGROUP_MODE_ENV, None)
        env.update(environment or {})
        self.process = subprocess.Popen(
            [sys.executable, str(driver), str(PY_ROOT),
             json.dumps([sys.executable, str(tree), str(self.pids_path), mode]), str(self.record_path)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
        )
        descendants.add(self.process.pid)
        assert self.process.stdout is not None
        line = self.process.stdout.readline()
        assert line.startswith("spawned"), (line, self.process.stderr.read() if self.process.stderr else "")
        assert _wait_until(self.pids_path.exists), "harness tree never reported"
        self.pids: dict[str, int] = json.loads(self.pids_path.read_text())
        for pid in self.pids.values():
            descendants.add(pid)
        self.record = ownership.parse_harness_record(json.loads(self.record_path.read_text()))

    def command(self, word: str) -> dict[str, object]:
        assert self.process.stdin is not None and self.process.stdout is not None
        self.process.stdin.write(word + "\n")
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        assert line, self.process.stderr.read() if self.process.stderr else ""
        value: object = json.loads(line)
        assert isinstance(value, dict)
        return value

    def crash(self) -> None:
        self.process.kill()
        self.process.communicate(timeout=10)

    def close(self) -> None:
        if self.process.poll() is None:
            with contextlib.suppress(BrokenPipeError):
                assert self.process.stdin is not None
                self.process.stdin.write("\n")
                self.process.stdin.flush()
            try:
                self.process.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.communicate(timeout=10)


class _RunnerFactory(Protocol):
    def __call__(self, mode: str, *, environment: dict[str, str] | None = None) -> _Runner: ...


@pytest.fixture()
def runner_factory(tmp_path: Path, descendants: _Descendants) -> Iterator[_RunnerFactory]:
    created: list[_Runner] = []

    def make(mode: str, *, environment: dict[str, str] | None = None) -> _Runner:
        directory = tmp_path / f"runner-{len(created)}"
        directory.mkdir()
        runner = _Runner(directory, descendants, mode, environment=environment)
        created.append(runner)
        return runner

    try:
        yield make
    finally:
        for runner in created:
            runner.close()


def _all_gone(descendants: _Descendants, pids: dict[str, int], timeout: float = 5.0) -> list[str]:
    """Labels of tracked processes still alive after ``timeout``."""
    deadline = time.monotonic() + timeout
    while True:
        alive = [label for label, pid in pids.items() if descendants.alive(pid)]
        if not alive or time.monotonic() >= deadline:
            return alive
        time.sleep(0.02)


def _gone(descendants: _Descendants, pid: int, timeout: float = 5.0) -> bool:
    return not _all_gone(descendants, {"process": pid}, timeout)


def _generation_holders(generation: str) -> list[int]:
    holders = []
    for stat in ownership._scan_processes():
        if stat.state not in ("Z", "X", "x") and ownership.carries_generation(stat.pid, generation):
            holders.append(stat.pid)
    return holders


# --------------------------------------------------------------------------- #
# Pinning and same-boot identity
# --------------------------------------------------------------------------- #


def test_pin_requires_the_recorded_boot_and_start_tick(child: subprocess.Popen[bytes]) -> None:
    exact = _identity(child.pid)
    pinned = ownership.pin(exact)
    assert pinned.verdict == ownership.ALIVE and pinned.process is not None
    pinned.process.close()

    stale = ownership.ProcessIdentity(child.pid, str(int(exact.started_at or "0") + 1), exact.boot_id)
    assert ownership.pin(stale).verdict == ownership.GONE

    other_boot = ownership.ProcessIdentity(child.pid, exact.started_at, "00000000-0000-4000-8000-000000000000")
    result = ownership.pin(other_boot)
    assert result.verdict == ownership.GONE and "earlier boot" in result.reason

    legacy = ownership.ProcessIdentity(child.pid, exact.started_at, None)
    assert ownership.pin(legacy).verdict == ownership.UNPROVEN
    proven = ownership.pin(legacy, legacy_proof=lambda pid, _stat: pid == child.pid)
    assert proven.verdict == ownership.ALIVE and proven.process is not None
    proven.process.close()

    without_start = ownership.ProcessIdentity(child.pid, None, exact.boot_id)
    assert ownership.pin(without_start).verdict == ownership.UNPROVEN
    assert _running(child.pid)


def test_stop_process_never_signals_a_reused_pid_or_an_earlier_boot(child: subprocess.Popen[bytes]) -> None:
    """Adversarial: the recorded number now names an unrelated live process."""
    exact = _identity(child.pid)
    for recorded in (
        ownership.ProcessIdentity(child.pid, str(int(exact.started_at or "0") - 1), exact.boot_id),
        ownership.ProcessIdentity(child.pid, exact.started_at, "00000000-0000-4000-8000-000000000000"),
    ):
        assert ownership.stop_process(recorded, grace=0) is False
        assert lib.terminate_runner_identity(recorded, grace=0) is False
    time.sleep(0.05)
    assert _running(child.pid, int(exact.started_at or "0"))


def test_stop_process_terminates_then_kills_through_the_pidfd(descendants: _Descendants) -> None:
    stubborn = subprocess.Popen(
        [sys.executable, "-c", "import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
         "print('ready', flush=True); time.sleep(30)"],
        stdout=subprocess.PIPE, text=True,
    )
    descendants.add(stubborn.pid)
    assert stubborn.stdout is not None and stubborn.stdout.readline() == "ready\n"
    started = time.monotonic()
    assert ownership.stop_process(_identity(stubborn.pid), grace=0.3) is True
    assert stubborn.wait(timeout=5) == -signal.SIGKILL
    assert time.monotonic() - started >= 0.3


def test_a_process_that_outlives_sigkill_is_reported_not_forgotten(
    child: subprocess.Popen[bytes], monkeypatch: pytest.MonkeyPatch,
) -> None:
    """D-state stand-in: the pidfd never reports an exit within the bound."""
    monkeypatch.setattr(ownership.PinnedProcess, "wait", lambda _self, _timeout: False)
    monkeypatch.setattr(ownership.PinnedProcess, "send", lambda _self, _signum: True)
    with pytest.raises(lib.AgentOperationError) as caught:
        ownership.stop_process(_identity(child.pid), grace=0, kill_wait=0.05)
    assert caught.value.code == "runner_stop_incomplete"
    assert f"pid {child.pid}" in caught.value.message


def test_unsupported_pidfds_refuse_rather_than_signal_by_number(
    child: subprocess.Popen[bytes], monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(ownership, "pidfds_supported", lambda: False)
    signals: list[tuple[int, int]] = []
    monkeypatch.setattr(os, "kill", lambda pid, signum: signals.append((pid, signum)))
    monkeypatch.setattr(os, "killpg", lambda pid, signum: signals.append((pid, signum)))
    identity = _identity(child.pid)
    with pytest.raises(lib.AgentOperationError) as caught:
        ownership.stop_process(identity, grace=0)
    assert caught.value.code == "pidfd_unsupported"
    record = ownership.HarnessRecord(
        runner=ownership.ProcessIdentity(os.getpid(), str(_identity(os.getpid()).started_at), identity.boot_id),
        leader=identity, process_group=child.pid, generation=secrets.token_hex(16),
        containment=ownership.CONTAINMENT_PROCESS,
    )
    with pytest.raises(lib.AgentOperationError) as tree:
        ownership.kill_harness_tree(record)
    assert tree.value.code == "pidfd_unsupported"
    assert signals == []
    # A positively dead identity needs no pidfd to be recognized as gone.
    assert ownership.stop_process(
        ownership.ProcessIdentity(child.pid, str(int(identity.started_at or "0") + 1), identity.boot_id),
        grace=0,
    ) is False
    assert _running(child.pid)


def test_unreadable_boot_identity_is_not_a_death_certificate(
    child: subprocess.Popen[bytes], monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
) -> None:
    fake_proc = tmp_path / "proc"
    (fake_proc / "sys/kernel/random").mkdir(parents=True)
    (fake_proc / "sys/kernel/random/boot_id").write_text("not-a-uuid\n")
    monkeypatch.setattr(ownership, "PROC", fake_proc)
    with pytest.raises(lib.AgentOperationError) as caught:
        ownership.pin(ownership.ProcessIdentity(child.pid, "1", "00000000-0000-4000-8000-000000000000"))
    assert caught.value.code == "runner_identity_unknown"


# --------------------------------------------------------------------------- #
# Records
# --------------------------------------------------------------------------- #


def test_harness_records_round_trip_and_reject_malformed_ownership(tmp_path: Path) -> None:
    generation = secrets.token_hex(16)
    boot = ownership.current_boot_id()
    record = ownership.HarnessRecord(
        runner=ownership.ProcessIdentity(10, "100", boot), leader=ownership.ProcessIdentity(11, "101", boot),
        process_group=11, generation=generation, containment=ownership.CONTAINMENT_PROCESS,
    )
    document = record.document("now")
    assert {"runner_pid", "runner_started_at", "pid", "started_at"} <= set(document)
    assert ownership.parse_harness_record(json.loads(json.dumps(document))) == record
    legacy = ownership.parse_harness_record(
        {"runner_pid": 10, "runner_started_at": "100", "pid": 11, "started_at": "101"})
    assert legacy.legacy and legacy.generation is None and legacy.leader.boot_id is None
    broken_documents: list[object] = [
        {**document, "generation": "short"},
        {**document, "boot_id": "nope"},
        {**document, "containment": "cgroup"},
        {**document, "containment": "cgroup", "cgroup": "/sys/fs/cgroup/other"},
        {**document, "containment": "cgroup", "cgroup": f"relative/agentctl-harness-{generation}"},
        {**document, "cgroup": f"/sys/fs/cgroup/agentctl-harness-{generation}"},
        {**document, "schema": "agentctl-headless-harness/v9"},
        {**document, "pid": True},
        [],
    ]
    for broken in broken_documents:
        with pytest.raises(lib.AgentOperationError) as caught:
            ownership.parse_harness_record(broken)
        assert caught.value.code == "harness_identity_unknown"


# --------------------------------------------------------------------------- #
# Harness trees
# --------------------------------------------------------------------------- #


def test_spawn_holds_the_harness_inert_until_its_record_is_durable(tmp_path: Path) -> None:
    marker = tmp_path / "executed"
    observed: list[str] = []

    def record(value: ownership.HarnessRecord) -> None:
        cmdline = (Path("/proc") / str(value.leader.pid) / "cmdline").read_bytes().split(b"\0")
        observed.append(os.fsdecode(cmdline[1]))
        assert not marker.exists()

    runner = ownership.ProcessIdentity(os.getpid(), str(_identity(os.getpid()).started_at),
                                       ownership.current_boot_id())
    script = f"from pathlib import Path; Path({str(marker)!r}).write_text('ran')"
    proc = ownership.spawn_owned_harness([sys.executable, "-c", script], cwd=str(tmp_path), stdin=False,
                                         runner=runner, record=record)
    assert proc.wait(timeout=10) == 0
    assert proc.finish()
    assert observed and observed[0].endswith("_launch_gate.py")
    assert marker.read_text() == "ran"

    def refuse(_value: ownership.HarnessRecord) -> None:
        raise OSError("disk full")

    with pytest.raises(OSError, match="disk full"):
        ownership.spawn_owned_harness([sys.executable, "-c", script.replace("ran", "late")],
                                      cwd=str(tmp_path), stdin=False, runner=runner, record=refuse)
    assert marker.read_text() == "ran"
    with pytest.raises(FileNotFoundError):
        ownership.spawn_owned_harness(["definitely-not-an-installed-harness"], cwd=str(tmp_path),
                                      stdin=False, runner=runner, record=record)


def test_runner_kill_tree_reaches_session_members_escapees_and_orphans(
    runner_factory: _RunnerFactory, descendants: _Descendants,
) -> None:
    runner = runner_factory("sleep")
    result = runner.command("kill_tree")
    assert result["complete"] is True, result
    assert _all_gone(descendants, runner.pids) == []


def test_leftovers_are_swept_before_the_exited_leader_is_reaped(
    runner_factory: _RunnerFactory, descendants: _Descendants,
) -> None:
    """Leader-dead/child-live inside the runner: the zombie leader pins its session."""
    runner = runner_factory("exit")
    result = runner.command("wait")
    assert result["returncode"] == 0 and result["complete"] is True, result
    assert int(str(result["killed"])) >= 4
    assert _all_gone(descendants, runner.pids) == []


def test_another_process_cleans_a_crashed_runners_tree_by_proof_only(
    runner_factory: _RunnerFactory, descendants: _Descendants,
) -> None:
    """Runner and leader are dead; only the generation still proves membership."""
    runner = runner_factory("sleep")
    runner.crash()
    pids = runner.pids
    # The leader carries the runner's parent-death signal and dies with it.
    assert _gone(descendants, pids["leader"])
    assert all(descendants.alive(pids[label]) for label in (
        "session_child", "escapee_with_generation", "session_without_generation",
        "double_fork_without_generation"))
    report = ownership.kill_harness_tree(runner.record, wait_s=5)
    assert report.complete, report.describe()
    assert _gone(descendants, pids["session_child"])
    assert _gone(descendants, pids["escapee_with_generation"])
    # Same session number, but the leader no longer holds it: not proven, not signalled.
    time.sleep(0.1)
    assert descendants.alive(pids["session_without_generation"])
    assert any(f"pid {pids['session_without_generation']}" in item for item in report.unproven)
    assert descendants.alive(pids["double_fork_without_generation"])


def test_another_process_cleans_a_live_runners_tree(
    runner_factory: _RunnerFactory, descendants: _Descendants,
) -> None:
    runner = runner_factory("sleep")
    report = ownership.kill_harness_tree(runner.record, wait_s=5)
    assert report.complete and report.converged, report.describe()
    assert _all_gone(descendants, runner.pids) == []


def test_without_pidfds_the_runner_peels_only_its_own_unreaped_children(
    runner_factory: _RunnerFactory, descendants: _Descendants,
) -> None:
    runner = runner_factory("sleep", environment={"TEST_NO_PIDFD": "1"})
    result = runner.command("kill_tree")
    assert result["complete"] is True, result
    assert _all_gone(descendants, runner.pids) == []


def test_a_recycled_session_number_is_never_signalled(child: subprocess.Popen[bytes]) -> None:
    """Adversarial: the recorded leader is gone and its number now leads an unrelated session."""
    exact = _identity(child.pid)
    record = ownership.HarnessRecord(
        runner=ownership.ProcessIdentity(1, "1", exact.boot_id),
        leader=ownership.ProcessIdentity(child.pid, str(int(exact.started_at or "0") - 1), exact.boot_id),
        process_group=child.pid, generation=secrets.token_hex(16),
        containment=ownership.CONTAINMENT_PROCESS,
    )
    report = ownership.kill_harness_tree(record, wait_s=1)
    assert report.complete and report.killed == 0
    assert any(f"pid {child.pid}" in item for item in report.unproven)
    time.sleep(0.05)
    assert _running(child.pid, int(exact.started_at or "0"))


_FORKER = r'''
import os, sys, time
out = sys.argv[1]
spawners = []
for _ in range(4):
    pid = os.fork()
    if pid == 0:
        for _ in range(40):
            if os.fork() == 0:
                time.sleep(30)
                os._exit(0)
            time.sleep(0.01)
        time.sleep(30)
        os._exit(0)
    spawners.append(pid)
open(out, "w").write(" ".join(map(str, spawners)))
time.sleep(30)
'''


def test_processes_forked_during_the_census_are_still_killed(
    tmp_path: Path, descendants: _Descendants,
) -> None:
    script = tmp_path / "forker.py"
    script.write_text(_FORKER)
    started = tmp_path / "started"
    # A recorded runner that is provably gone, so "child of the runner" (here,
    # of pytest) never applies; the generation and session proofs must suffice.
    runner_identity = ownership.ProcessIdentity(os.getpid(), "1", ownership.current_boot_id())
    records: list[ownership.HarnessRecord] = []
    proc = ownership.spawn_owned_harness([sys.executable, str(script), str(started)], cwd=str(tmp_path),
                                         stdin=False, runner=runner_identity, record=records.append)
    descendants.add(proc.pid)
    try:
        assert _wait_until(started.exists)
        generation = records[0].generation
        assert generation is not None
        # Cleanup from outside the runner while all four spawners keep forking.
        report = ownership.kill_harness_tree(records[0], wait_s=8)
        assert report.complete, report.describe()
        assert _wait_until(lambda: not _generation_holders(generation), 5), _generation_holders(generation)
    finally:
        proc.kill()
        proc.wait(timeout=10)
        for holder in _generation_holders(records[0].generation or "-"):
            descendants.add(holder)


def test_incomplete_cleanup_keeps_the_record_and_says_what_remains(
    runner_factory: _RunnerFactory, descendants: _Descendants,
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
) -> None:
    runner = runner_factory("sleep")
    survivor = runner.pids["session_child"]
    real_exited = ownership.PinnedProcess.exited
    real_wait = ownership.PinnedProcess.wait

    def exited(self: ownership.PinnedProcess) -> bool:
        return False if self.pid == survivor else real_exited(self)

    def wait(self: ownership.PinnedProcess, timeout: float | None) -> bool:
        return False if self.pid == survivor else real_wait(self, timeout)

    monkeypatch.setattr(ownership.PinnedProcess, "exited", exited)
    monkeypatch.setattr(ownership.PinnedProcess, "wait", wait)
    base = tmp_path / "subagents"
    monkeypatch.setattr(lib, "BASE", base)
    monkeypatch.setattr(lib, "STATE", base / "state")
    monkeypatch.setattr(lib, "REGISTRY", base / "registry.json")
    monkeypatch.setattr(lib, "LOCKFILE", base / ".registry.lock")
    lib.ensure_agent_dirs("stuck")
    lib.active_harness_path("stuck").write_text(runner.record_path.read_text())
    rec = lib.AgentRecord(
        name="stuck", harness="codex", backend="tmux", tmux_target="subagents:stuck", cwd=str(tmp_path),
        model=None, session_id=None, status="busy", runner_pid=runner.record.runner.pid,
        runner_started_at=runner.record.runner.started_at, next_seq=1, created_at=lib.now_iso(),
        last_turn_at=None, runner_boot_id=runner.record.runner.boot_id,
    )
    monkeypatch.setattr(ownership, "KILL_WAIT_S", 0.5)
    with pytest.raises(lib.AgentOperationError) as caught:
        lib.terminate_active_harness(rec)
    assert caught.value.code == "harness_cleanup_incomplete"
    assert f"pid {survivor}" in caught.value.message
    assert lib.active_harness_path("stuck").exists()
    monkeypatch.undo()


def test_cgroup_containment_kills_what_no_process_proof_can_reach(
    runner_factory: _RunnerFactory, descendants: _Descendants,
) -> None:
    parent = ownership.own_cgroup_directory()
    if parent is None or not (parent / "cgroup.kill").exists() or not os.access(parent, os.W_OK):
        pytest.skip("no writable cgroup v2 directory with cgroup.kill for this test process")
    probe = parent / f"{ownership.CGROUP_PREFIX}{secrets.token_hex(16)}"
    try:
        probe.mkdir()
    except OSError as exc:
        pytest.skip(f"cannot create a child cgroup here: {exc}")
    probe.rmdir()
    runner = runner_factory("sleep", environment={ownership.CGROUP_MODE_ENV: "require"})
    assert runner.record.containment == ownership.CONTAINMENT_CGROUP and runner.record.cgroup is not None
    runner.crash()
    orphan = runner.pids["double_fork_without_generation"]
    assert descendants.alive(orphan)
    report = ownership.kill_harness_tree(runner.record, wait_s=5)
    assert report.complete, report.describe()
    assert _all_gone(descendants, runner.pids) == []
    assert not runner.record.cgroup.exists()


def test_cgroup_require_fails_closed_and_auto_falls_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(ownership, "own_cgroup_directory", lambda: tmp_path / "missing" / "cgroup")
    runner = _identity(os.getpid())
    notes: list[str] = []
    records: list[ownership.HarnessRecord] = []
    monkeypatch.setenv(ownership.CGROUP_MODE_ENV, "auto")
    proc = ownership.spawn_owned_harness([sys.executable, "-c", "pass"], cwd=str(tmp_path), stdin=False,
                                         runner=runner, record=records.append, on_cgroup_fallback=notes.append)
    assert proc.wait(timeout=10) == 0 and proc.finish()
    assert records[0].containment == ownership.CONTAINMENT_PROCESS and notes
    monkeypatch.setenv(ownership.CGROUP_MODE_ENV, "require")
    with pytest.raises(lib.AgentOperationError) as caught:
        ownership.spawn_owned_harness([sys.executable, "-c", "pass"], cwd=str(tmp_path), stdin=False,
                                      runner=runner, record=records.append)
    assert caught.value.code == "harness_cgroup_unavailable"
    monkeypatch.setenv(ownership.CGROUP_MODE_ENV, "sometimes")
    with pytest.raises(lib.AgentOperationError) as invalid:
        ownership.cgroup_mode()
    assert invalid.value.code == "invalid_cgroup_mode"


def test_runtime_never_signals_by_number_outside_the_parent_pinned_fallback() -> None:
    """Every numeric kill left in the runtime targets this process's own unreaped child.

    The scan matches any ``.kill``/``.killpg``/``.send_signal``/``.terminate``
    call, whatever the receiver, so a numeric fallback cannot hide behind
    ``Popen.kill`` or ``super().send_signal``. The allowlist is the complete
    account: the two parent-pinned ``os.kill`` sites, ``OwnedHarnessProcess``
    dispatching to its own verified ``send_signal``, and the runner's
    test-double branch (production harnesses are always owned processes and
    take ``kill_tree``).
    """
    allowed = {
        ("ownership.py", "send"),
        ("ownership.py", "_peel_children"),
        ("ownership.py", "terminate"),
        ("ownership.py", "kill"),
        ("agent_runner.py", "_kill_harness"),
    }
    package = Path(ownership.__file__).resolve().parent
    found: list[tuple[str, str, int]] = []
    for path in sorted(package.rglob("*.py")):
        if "tests" in path.parts:
            continue
        tree = ast.parse(path.read_text(), str(path))
        for function in ast.walk(tree):
            if not isinstance(function, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for node in ast.walk(function):
                if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                        and node.func.attr in ("kill", "killpg", "send_signal", "terminate")):
                    found.append((path.name, function.name, node.lineno))
    unexpected = [item for item in found if (item[0], item[1]) not in allowed]
    assert unexpected == [], unexpected
    assert {(name, function) for name, function, _line in found} <= allowed


# --------------------------------------------------------------------------- #
# The corpus shared with the other edition's retirement proof
# --------------------------------------------------------------------------- #

CORPUS = PY_ROOT.parent / "rs" / "agentctl" / "testdata" / "headless-ownership" / "cases.json"
_CORPUS_ROW: dict[str, object] = {
    "name": "worker", "harness": "codex", "backend": "tmux", "tmux_target": "subagents:worker",
    "cwd": "/work/project", "model": None, "session_id": "thread", "status": "idle", "runner_pid": 4242,
    "runner_started_at": "555", "next_seq": 3, "created_at": "2026-10-01T00:00:00+00:00",
    "last_turn_at": None, "mode": "headless", "presentation_pane": None,
    "codex_bypass_permissions": False, "harness_args": [],
}


def _corpus() -> dict[str, object]:
    value: object = json.loads(CORPUS.read_text())
    assert isinstance(value, dict)
    return value


def _corpus_cases() -> list[dict[str, object]]:
    cases = _corpus()["cases"]
    assert isinstance(cases, list)
    return [case for case in cases if isinstance(case, dict)]


def _stat_line(pid: int, start: int, state: str) -> str:
    return f"{pid} (corpus (proc)) {state} 1 {pid} {pid} 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 {start} 1000 10\n"


@pytest.mark.parametrize("case", _corpus_cases(), ids=lambda case: str(case["name"]))
def test_shared_corpus_verdicts(
    case: dict[str, object], tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    corpus = _corpus()
    boot, other_boot = str(corpus["boot"]), str(corpus["other_boot"])
    cgroup_root = tmp_path / "cgroup"
    cgroup_root.mkdir()

    def expand(text: str) -> str:
        return (text.replace("$OTHER_BOOT", other_boot).replace("$BOOT", boot)
                .replace("$CGROUP", str(cgroup_root)))

    fake_proc = tmp_path / "proc"
    (fake_proc / "self").mkdir(parents=True)
    (fake_proc / "self" / "status").write_text("Name:\tagentctl\nNSpid:\t100\n")
    if case.get("boot", "current") is not None:
        (fake_proc / "sys/kernel/random").mkdir(parents=True)
        (fake_proc / "sys/kernel/random/boot_id").write_text(boot + "\n")
    processes = case["processes"]
    assert isinstance(processes, list)
    for entry in processes:
        assert isinstance(entry, dict)
        pid, start, process_state = int(entry["pid"]), int(entry["start"]), str(entry["state"])
        (fake_proc / str(pid)).mkdir()
        (fake_proc / str(pid) / "stat").write_text(_stat_line(pid, start, process_state))
        environ = entry.get("environ")
        if isinstance(environ, list):
            (fake_proc / str(pid) / "environ").write_bytes(
                b"".join(str(item).encode() + b"\0" for item in environ))
    cgroups = case["cgroups"]
    assert isinstance(cgroups, dict)
    for directory, events in cgroups.items():
        (cgroup_root / str(directory)).mkdir()
        (cgroup_root / str(directory) / "cgroup.events").write_text(str(events))
    home = tmp_path / "runtime"
    state = home / "state" / "worker"
    state.mkdir(parents=True)
    rows = [] if case.get("row", "default") is None else [_CORPUS_ROW]
    (home / "registry.json").write_text(json.dumps(rows))
    files = case["files"]
    assert isinstance(files, dict)
    for file_name, content in files.items():
        text = content if isinstance(content, str) else json.dumps(content)
        (state / str(file_name)).write_text(expand(text))
    monkeypatch.setattr(ownership, "PROC", fake_proc)
    monkeypatch.setattr(lib, "BASE", home)
    monkeypatch.setattr(lib, "STATE", home / "state")
    monkeypatch.setattr(lib, "REGISTRY", home / "registry.json")
    monkeypatch.setattr(lib, "LOCKFILE", home / ".registry.lock")
    verdict, detail = lib.recorded_liveness("worker")
    assert verdict == case["expect"], f"{case['name']}: {case['why']}: {detail}"
