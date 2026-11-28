"""A peer's in-flight registry write is never reported as a leftover (#2352).

Every writer of a registry temporary file (``<sequence>.json.tmp.<pid>.<hex>``
in an event log, a shard or journal ``.tmp`` at the control top level) holds
the exclusive mutation locks from creating the file until its rename. A
prevalidated mutation used to run the partial-state scan in its lock-free
round and report what it found once the registry identity read the same
twice. A peer that sits between its fsync and its rename leaves the identity
unchanged, so a heartbeat on a loaded host refused, rc 3, "partial atomic
update found ... use 'wrkslots recover'", naming another slot's live write.

A lock-free finding now defers to the locked round: a file the peer renames
or removes changes the identity, and the next round validates. A file still
there under the locks, where no live writer can hold it, refuses exactly as
before, with nothing written. Every prevalidated command (create, adopt,
transfer-owner, heartbeat) is covered, and both editions' heartbeats meet a
real peer process that holds the locks across the round. Conditions only,
never a wall-clock budget: the peer releases once ``/proc/locks`` shows the
heartbeat queued behind it.
"""

from __future__ import annotations

import contextlib
import fcntl
import json
import os
import subprocess
import sys
import time
import uuid
from collections.abc import Iterator
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_lifecycle import (
    _absent_batch_refused_after_its_archives,
    active_slots,
    command,
    control_directory,
    create,
    make_project,
    mark_owner_dead,
    source_environment,
)

AU_ROOT = Path(__file__).resolve().parents[3]
PY_ROOT = AU_ROOT / "py"
COMMANDS = ("create", "adopt", "transfer-owner", "heartbeat")
#: A hang guard for the condition waits, never a budget a passing run spends.
HANG_GUARD_SECONDS = 900.0


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")
    monkeypatch.setenv("WRKSLOTS_LOCK_ORDER_CHECK", "1")


def _events(project: Path) -> Path:
    return control_directory(project) / "EVENTS.testhost"


def _event_count(project: Path) -> int:
    return len(list(_events(project).glob("*.json")))


def _next_sequence(project: Path) -> int:
    return max(int(path.name.split(".")[0]) for path in _events(project).glob("*.json")) + 1


def _peer_temp(project: Path, pid: int) -> Path:
    """The temporary file a peer's next event write holds before its rename."""
    return _events(project) / (
        f"{_next_sequence(project):020d}.json.tmp.{pid}.{uuid.uuid4().hex}"
    )


def _row(project: Path, slot: str) -> dict[str, object]:
    for row in active_slots(project):
        assert isinstance(row, dict)
        if row["slot"] == slot:
            return row
    raise AssertionError(f"no row for {slot}")


def _exclusive_lock_held(project: Path) -> bool:
    registry = control_directory(project) / "ACTIVE.global.lock"
    if not registry.exists():
        return False
    fd = os.open(registry, os.O_RDONLY | os.O_CLOEXEC)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    finally:
        os.close(fd)


def _project_for(tmp_path: Path, name: str) -> Path:
    """A project whose slot01 is ready for ``name``; slot02 is a bystander."""
    project, _repository, _remote = make_project(tmp_path)
    if name != "create":
        made = create(project, bind_owner=name != "adopt")
        assert made.returncode == 0, made.stderr
    made = create(project, slot="slot02", agent="codex-2", branch="codex/task2")
    assert made.returncode == 0, made.stderr
    if name == "transfer-owner":
        mark_owner_dead(project, slot="slot01")
    return project


def _argv(project: Path, name: str) -> list[str]:
    pid = str(os.getpid())
    owner = ["--agent", "codex-1", "--owner-pid", pid]
    if name == "create":
        tail = [
            "create", "slot01", "--slot-type", "agent", "--coordinator-authorized",
            *owner, "--task", "task-slot01", "--purpose", "test slot01",
            "--coordinator-pid", pid, "--repo", "product=repo",
            "--branch", "product=codex/task",
        ]
    else:
        tail = [name, "slot01", *owner, "--expected-generation", "1"]
    return ["--project-root", str(project), *tail]


@contextlib.contextmanager
def _peer_finishes_at_the_lock(
    monkeypatch: pytest.MonkeyPatch, temp: Path
) -> Iterator[list[bool]]:
    """Remove ``temp`` as this command queues for its exclusive locks.

    The removal stands in for the peer's rename or cleanup, which the peer
    finishes before it releases the locks this command then takes.
    """
    locks = cli._mutation_locks
    cleared: list[bool] = []

    @contextlib.contextmanager
    def peer_then_locks(
        config: cli.Config,
        wait_seconds: float,
        *,
        deadline: float | None = None,
        shared: bool = False,
        release_config_after_check: bool = False,
    ) -> Iterator[None]:
        if not shared and temp.exists():
            temp.unlink()
            cleared.append(True)
        with locks(
            config,
            wait_seconds,
            deadline=deadline,
            shared=shared,
            release_config_after_check=release_config_after_check,
        ):
            yield

    monkeypatch.setattr(cli, "_mutation_locks", peer_then_locks)
    yield cleared


@pytest.mark.parametrize("name", COMMANDS)
def test_a_peer_temp_gone_by_the_lock_never_refuses(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    name: str,
) -> None:
    project = _project_for(tmp_path, name)
    temp = _peer_temp(project, os.getpid())
    temp.write_text('{"partial": ', encoding="utf-8")
    events = _event_count(project)

    with _peer_finishes_at_the_lock(monkeypatch, temp) as cleared:
        rc = cli.main(_argv(project, name))
    captured = capsys.readouterr()

    assert rc == 0, captured.err
    assert "partial atomic update" not in captured.err
    assert cleared == [True]
    assert not temp.exists()
    assert _event_count(project) > events
    assert _row(project, "slot01")["agent"] == "codex-1"


@pytest.mark.parametrize("name", COMMANDS)
def test_a_stale_temp_still_refuses_under_the_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    name: str,
) -> None:
    project = _project_for(tmp_path, name)
    temp = _peer_temp(project, os.getpid())
    temp.write_text('{"partial": ', encoding="utf-8")
    shard = control_directory(project) / "ACTIVE.testhost.json"
    shard_before = shard.read_bytes()
    events = _event_count(project)
    judged: list[bool] = []
    scan = cli._refuse_partial_state

    def recorded(config: cli.Config, **options: bool) -> None:
        try:
            scan(config, **options)
        except cli.StateError as exc:
            if "partial atomic update found" in str(exc):
                judged.append(_exclusive_lock_held(project))
            raise

    monkeypatch.setattr(cli, "_refuse_partial_state", recorded)

    rc = cli.main(_argv(project, name))
    captured = capsys.readouterr()

    assert rc == 3
    assert (
        f"partial atomic update found: {temp}; preserve it and use 'wrkslots recover'"
        in captured.err
    )
    # The refusal that reached the caller was judged with the exclusive
    # lock held, where no live writer can still own the file.
    assert judged and judged[-1] is True
    assert temp.exists()
    assert _event_count(project) == events
    assert shard.read_bytes() == shard_before


_PEER = r"""
import os, sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from wrkslots import cli
project, ready, release = Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4])
config = cli._load_config(str(project), None)
with cli._mutation_locks(config, 60.0, release_config_after_check=True):
    events = config.control / "EVENTS.testhost"
    sequence = max(int(p.name.split(".")[0]) for p in events.glob("*.json")) + 1
    temp = events / f"{sequence:020d}.json.tmp.{os.getpid()}.{os.urandom(16).hex()}"
    temp.write_text('{"partial": ', encoding="utf-8")
    ready.write_text(str(temp), encoding="utf-8")
    while not release.exists():
        time.sleep(0.02)
    temp.unlink()
"""


def _lock_inodes(project: Path) -> set[int]:
    return {
        path.stat().st_ino
        for path in control_directory(project).glob("ACTIVE.*.lock")
    }


def _queued_behind_the_peer(project: Path) -> bool:
    """Does ``/proc/locks`` show a process blocked on a registry lock?"""
    inodes = _lock_inodes(project)
    for line in Path("/proc/locks").read_text(encoding="utf-8").splitlines():
        fields = line.split()
        if len(fields) > 5 and fields[1] == "->" and fields[2] == "FLOCK":
            if int(fields[6].rsplit(":", 1)[1]) in inodes:
                return True
    return False


def _wait_for(condition: str, check: object) -> None:
    assert callable(check)
    give_up = time.monotonic() + HANG_GUARD_SECONDS
    while not check():
        assert time.monotonic() < give_up, f"hang guard: {condition}"
        time.sleep(0.02)


def _slot_argv(name: str) -> list[str]:
    return [
        name, "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    ]


@pytest.mark.parametrize("name", ("heartbeat", "transfer-owner"))
@pytest.mark.parametrize("edition", ("python", "rust"))
def test_a_writer_queues_behind_a_real_peer_holding_its_temp(
    tmp_path: Path, edition: str, name: str
) -> None:
    """Both editions' lock-free prepares: a real peer holds the locks and a temp."""
    project = _project_for(tmp_path, name)
    ready, release = tmp_path / "peer-ready", tmp_path / "peer-release"
    peer = subprocess.Popen(
        [sys.executable, "-c", _PEER, str(PY_ROOT), str(project), str(ready), str(release)],
        env=source_environment(),
    )
    try:
        _wait_for("the peer holds the locks", ready.exists)
        temp = Path(ready.read_text(encoding="utf-8"))
        assert temp.exists()
        before = _row(project, "slot01")
        if edition == "rust":
            writer = subprocess.Popen(
                [str(rust_wrkslotsd()), *_slot_argv(name), "--root", str(project)],
                cwd=AU_ROOT, text=True,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                env=dict(
                    os.environ,
                    WRKSLOTSD_FIXTURE_WRITER="1",
                    WRKSLOTS_WAIT_LOCK=str(HANG_GUARD_SECONDS),
                ),
            )
        else:
            writer = subprocess.Popen(
                [
                    sys.executable, "-m", "wrkslots.tests.idle_validation_host",
                    "--project-root", str(project),
                    "--wait-lock", str(HANG_GUARD_SECONDS), *_slot_argv(name),
                ],
                text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                env=source_environment(),
            )
        # Before the fix the command refused at once; now it queues for the
        # locks the peer holds, and the peer finishes when it does.
        _wait_for(
            "the command queues behind the peer, or exits",
            lambda: writer.poll() is not None or _queued_behind_the_peer(project),
        )
        release.write_text("go", encoding="utf-8")
        stdout, stderr = writer.communicate(timeout=HANG_GUARD_SECONDS)
        assert peer.wait(timeout=HANG_GUARD_SECONDS) == 0
    finally:
        release.write_text("go", encoding="utf-8")
        with contextlib.suppress(subprocess.TimeoutExpired):
            peer.wait(timeout=60)
        if peer.poll() is None:
            peer.kill()

    assert writer.returncode == 0, stderr
    assert "partial atomic update" not in stderr
    assert not temp.exists()
    after = _row(project, "slot01")
    if name == "heartbeat":
        assert after["heartbeat_at"] != before["heartbeat_at"]
        if edition == "rust":
            report = json.loads(stdout)
            assert (report["slot"], report["generation"]) == ("slot01", 1)
        else:
            assert "heartbeat slot=slot01 generation=1" in stdout
    else:
        assert after != before
        assert after["agent"] == "codex-1"


@pytest.mark.parametrize("name", ("heartbeat", "transfer-owner"))
def test_the_rust_edition_refuses_a_stale_temp_after_taking_the_locks(
    tmp_path: Path, name: str
) -> None:
    project = _project_for(tmp_path, name)
    temp = _peer_temp(project, os.getpid())
    temp.write_text('{"partial": ', encoding="utf-8")
    shard = control_directory(project) / "ACTIVE.testhost.json"
    before = json.loads(shard.read_text(encoding="utf-8"))
    events = _event_count(project)

    done = subprocess.run(
        [str(rust_wrkslotsd()), *_slot_argv(name), "--root", str(project)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=HANG_GUARD_SECONDS,
        env=dict(os.environ, WRKSLOTSD_FIXTURE_WRITER="1"),
    )

    assert done.returncode != 0
    assert f"partial atomic update found: {temp}" in done.stderr
    assert temp.exists()
    assert _event_count(project) == events
    assert json.loads(shard.read_text(encoding="utf-8")) == before
    # The Python edition refuses the same scene with the same text.
    python = command(project, *_slot_argv(name))
    assert python.returncode == 3
    assert f"partial atomic update found: {temp}" in python.stderr


@pytest.mark.parametrize("name", COMMANDS)
def test_no_prevalidated_command_recovers_a_journal_without_the_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    name: str,
) -> None:
    """Dead-actor journal recovery writes, so it runs only under the lock (#2352).

    It appends events and rewrites shards; run lock-free, its temporary files
    could be live while another command holds the locks, which would break the
    rule that a temporary file under the locks is a leftover.
    """
    project = _project_for(tmp_path, name)
    calls: list[bool] = []
    recover = cli._recover_dead_actor_journals

    def recorded(config: cli.Config) -> None:
        calls.append(_exclusive_lock_held(project))
        recover(config)

    monkeypatch.setattr(cli, "_recover_dead_actor_journals", recorded)

    rc = cli.main(_argv(project, name))

    assert rc == 0, capsys.readouterr().err
    assert False not in calls


def test_a_heartbeat_recovers_a_dead_actors_journal_under_the_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """The heartbeat still recovers a dead actor's journal, now only locked (#912, #2352)."""
    project, _input_path, config = _absent_batch_refused_after_its_archives(
        tmp_path, monkeypatch
    )
    assert isinstance(config, cli.Config)
    made = create(project, slot="fresh-h", agent="agent-fresh-h", branch="agent/fresh-h")
    assert made.returncode == 0, made.stderr
    journal = control_directory(project) / "ACTIVE.testhost.journal"
    raw = json.loads(journal.read_text(encoding="utf-8"))
    raw["actor"] = {**raw["actor"], "pid": 2_147_483_647}
    cli._write_journal(config, raw)
    calls: list[bool] = []
    recover = cli._recover_dead_actor_journals

    def recorded(config: cli.Config) -> None:
        calls.append(_exclusive_lock_held(project))
        recover(config)

    monkeypatch.setattr(cli, "_recover_dead_actor_journals", recorded)
    capsys.readouterr()

    rc = cli.main([
        "--project-root", str(project), "heartbeat", "fresh-h",
        "--agent", "agent-fresh-h", "--owner-pid", str(os.getpid()),
        "--expected-generation", "1",
    ])
    err = capsys.readouterr().err

    assert rc == 0, err
    assert "recovered the dead actor's journal" in err
    assert not journal.exists()
    assert calls and False not in calls
