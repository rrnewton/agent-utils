"""No writer holds the configuration locks while queueing for the registry (#2000).

The prevalidated heartbeat, adopt and transfer-owner rounds (and their
busy fallbacks) check the configuration under its locks, release
them, and only then queue at the writer-intent turnstile for the
ACTIVE registry locks. These tests hold the registry lock shared,
standing in for a long reader, and prove a queued writer from any of
those commands holds the turnstile but neither configuration flock;
they also pin the env-gated lock-order assertion and the old-by-new
pairing against a pinned older revision, whose lock stack takes the
project-root flock (its only configuration lock) and the same intent
turnstile, in the same relative order, so the two orders interleave
without a cycle.
"""

from __future__ import annotations

import fcntl
import os
import subprocess
import sys
import time
from collections.abc import Callable, Iterator
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    control_directory,
    create,
    make_project,
    mark_owner_dead,
    source_environment,
)

AU_ROOT = Path(__file__).resolve().parents[3]
PINNED_OLD = "096a8d74"

_FLAGS_FILE = os.O_RDWR | os.O_CREAT | os.O_CLOEXEC
_FLAGS_DIR = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")
    monkeypatch.setenv("WRKSLOTS_LOCK_ORDER_CHECK", "1")


def _flock_free(path: Path, flags: int) -> bool:
    fd = os.open(path, flags, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return False
        fcntl.flock(fd, fcntl.LOCK_UN)
        return True
    finally:
        os.close(fd)


def _intent_held(control: Path) -> bool:
    """Whether a writer holds the turnstile: a shared probe then blocks."""
    fd = os.open(control / "ACTIVE.global.intent.lock", _FLAGS_FILE, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    finally:
        os.close(fd)


def _wait_for(
    condition: Callable[[], bool], description: str, *parties: subprocess.Popen[str]
) -> None:
    deadline = time.monotonic() + 120.0
    while not condition():
        for party in parties:
            assert party.poll() is None, (description, party.communicate())
        assert time.monotonic() < deadline, description
        time.sleep(0.05)


def _project_with_slots(tmp_path: Path) -> Path:
    project, _repository, _remote = make_project(tmp_path)
    for index in (1, 2):
        made = create(
            project,
            slot=f"slot{index:02d}",
            agent=f"codex-{index}",
            branch=f"codex/task{index}",
        )
        assert made.returncode == 0, made.stderr
    return project


def _spawn(
    project: Path, argv: list[str], *, old_tree: Path | None = None
) -> subprocess.Popen[str]:
    env = source_environment({"WRKSLOTS_LOCK_ORDER_CHECK": "1"})
    if old_tree is not None:
        env["PYTHONPATH"] = str(old_tree / "py")
        # Prove the old party really imports the pinned tree, never the
        # current code by accident (#2000).
        probe = subprocess.run(
            [sys.executable, "-c", "import wrkslots.cli; print(wrkslots.cli.__file__)"],
            text=True, capture_output=True, check=False, env=env,
            cwd=str(old_tree), timeout=60,
        )
        assert probe.returncode == 0, probe.stderr
        assert Path(probe.stdout.strip()).is_relative_to(old_tree), probe.stdout
    return subprocess.Popen(
        [
            sys.executable, "-m", "wrkslots.tests.idle_validation_host",
            "--project-root", str(project), "--wait-lock", "300", *argv,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        cwd=str(old_tree) if old_tree is not None else None,
    )


def _heartbeat_argv(slot: str, agent: str) -> list[str]:
    return [
        "heartbeat", slot, "--agent", agent,
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    ]


def _assert_queued_without_config(
    project: Path, argv: list[str], expected: str, *, old_tree: Path | None = None
) -> None:
    control = control_directory(project)
    reader = os.open(control / "ACTIVE.global.lock", _FLAGS_FILE, 0o600)
    fcntl.flock(reader, fcntl.LOCK_SH)
    party = _spawn(project, argv, old_tree=old_tree)
    try:
        _wait_for(lambda: _intent_held(control), "the writer never queued", party)
        assert _flock_free(project, _FLAGS_DIR), "project-root flock held while queued"
        config_lock = control / "config.lock"
        assert _flock_free(config_lock, _FLAGS_FILE), "config.lock held while queued"
    finally:
        os.close(reader)
    out, err = party.communicate(timeout=300)
    assert party.returncode == 0, err
    assert expected in out


def test_a_heartbeat_queued_for_the_registry_write_lock_holds_no_project_root_flock(
    tmp_path: Path,
) -> None:
    """#2000: while a heartbeat waits behind a registry reader, the project-root flock
    stays free for the other tools that take it. Before the fix the heartbeat held
    it for its whole wait."""
    project = _project_with_slots(tmp_path)
    _assert_queued_without_config(
        project, _heartbeat_argv("slot01", "codex-1"), "heartbeat slot=slot01 generation=1"
    )


def test_an_adopt_queued_for_the_registry_write_lock_holds_no_config_lock(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, bind_owner=False)
    assert made.returncode == 0, made.stderr
    _assert_queued_without_config(
        project,
        [
            "adopt", "slot01", "--agent", "codex-1",
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        ],
        "adopted slot=slot01",
    )


def test_a_transfer_queued_for_the_registry_write_lock_holds_no_config_lock(
    tmp_path: Path,
) -> None:
    project = _project_with_slots(tmp_path)
    mark_owner_dead(project, slot="slot01")
    _assert_queued_without_config(
        project,
        [
            "transfer-owner", "slot01", "--agent", "codex-1",
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        ],
        "transferred slot=slot01",
    )


def test_lock_order_assertion_trips_on_an_out_of_order_acquisition(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = cli._load_config(str(project), "testhost")
    subject = config.control / "ACTIVE"
    with cli._locked(subject, exclusive=True, wait_seconds=5.0):
        with pytest.raises(AssertionError, match="lock order violation"):
            with cli._locked_config(config.config_path, 5.0, control=config.control):
                pass


def test_lock_order_assertion_is_off_without_the_env_flag(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("WRKSLOTS_LOCK_ORDER_CHECK")
    project, _repository, _remote = make_project(tmp_path)
    config = cli._load_config(str(project), "testhost")
    subject = config.control / "ACTIVE"
    # The same out-of-order nesting is unchecked while the flag is off.
    with cli._locked(subject, exclusive=True, wait_seconds=5.0):
        with cli._locked_config(config.config_path, 5.0, control=config.control):
            pass


@pytest.fixture()
def old_tree(tmp_path: Path) -> Iterator[Path]:
    probe = subprocess.run(
        ["git", "-C", str(AU_ROOT), "cat-file", "-t", PINNED_OLD],
        capture_output=True, text=True, check=False, timeout=60,
    )
    if probe.returncode != 0:
        pytest.skip(f"pinned revision {PINNED_OLD} is unavailable in this checkout")
    tree = tmp_path / "old-wrkslots"
    tree.mkdir()
    # Materialise with `git archive` into the test tmp dir (#2000): a
    # `git worktree add` would mutate the shared repository's worktree
    # registry, which a test must never do.
    archived = subprocess.run(
        ["git", "-C", str(AU_ROOT), "archive", PINNED_OLD],
        capture_output=True, check=False, timeout=300,
    )
    if archived.returncode != 0:
        pytest.skip(
            f"cannot materialise pinned revision {PINNED_OLD}: "
            f"{archived.stderr.decode(errors='replace').strip()}"
        )
    extracted = subprocess.run(
        ["tar", "-x", "-C", str(tree)],
        input=archived.stdout, capture_output=True, check=False, timeout=300,
    )
    if extracted.returncode != 0:
        pytest.skip(
            f"cannot extract pinned revision {PINNED_OLD}: "
            f"{extracted.stderr.decode(errors='replace').strip()}"
        )
    yield tree


def _status_completes(project: Path, *, old_tree: Path | None = None) -> None:
    env = source_environment({"WRKSLOTS_LOCK_ORDER_CHECK": "1"})
    if old_tree is not None:
        env["PYTHONPATH"] = str(old_tree / "py")
    if old_tree is not None:
        probe = subprocess.run(
            [sys.executable, "-c", "import wrkslots.cli; print(wrkslots.cli.__file__)"],
            text=True, capture_output=True, check=False, env=env,
            cwd=str(old_tree), timeout=60,
        )
        assert probe.returncode == 0, probe.stderr
        assert Path(probe.stdout.strip()).is_relative_to(old_tree), probe.stdout
    result = subprocess.run(
        [
            sys.executable, "-m", "wrkslots.tests.idle_validation_host",
            "--project-root", str(project), "--wait-lock", "120", "status",
        ],
        text=True, capture_output=True, check=False, env=env, timeout=300,
        cwd=str(old_tree) if old_tree is not None else None,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("writer_old", (True, False))
def test_old_and_new_writer_reader_pairings_complete(
    tmp_path: Path, old_tree: Path, writer_old: bool
) -> None:
    """Old writer vs new reader, and new writer vs old reader (#2000).

    The writer queues behind a shared registry reader; the other
    edition's status read completes meanwhile (its turnstile deferral
    is bounded), then every party completes once the reader releases.
    The writer_old=True pairing is a compatibility check only: the old
    binary holds its configuration flock while queueing by design, so
    it proves the two orders interleave without deadlock, not the fix.
    """
    project = _project_with_slots(tmp_path)
    control = control_directory(project)
    reader = os.open(control / "ACTIVE.global.lock", _FLAGS_FILE, 0o600)
    fcntl.flock(reader, fcntl.LOCK_SH)
    writer = _spawn(
        project,
        _heartbeat_argv("slot01", "codex-1"),
        old_tree=old_tree if writer_old else None,
    )
    try:
        _wait_for(lambda: _intent_held(control), "the writer never queued", writer)
        if not writer_old:
            assert _flock_free(project, _FLAGS_DIR)
        _status_completes(project, old_tree=None if writer_old else old_tree)
        assert writer.poll() is None
    finally:
        os.close(reader)
    out, err = writer.communicate(timeout=300)
    assert writer.returncode == 0, err
    assert "heartbeat slot=slot01 generation=1" in out


def test_old_and_new_heartbeats_racing_one_registry_both_complete(
    tmp_path: Path, old_tree: Path
) -> None:
    project = _project_with_slots(tmp_path)
    control = control_directory(project)
    reader = os.open(control / "ACTIVE.global.lock", _FLAGS_FILE, 0o600)
    fcntl.flock(reader, fcntl.LOCK_SH)
    parties = [
        _spawn(project, _heartbeat_argv("slot01", "codex-1"), old_tree=old_tree),
        _spawn(project, _heartbeat_argv("slot02", "codex-2")),
    ]
    try:
        _wait_for(lambda: _intent_held(control), "no heartbeat queued", *parties)
    finally:
        os.close(reader)
    for party, slot in zip(parties, ("slot01", "slot02"), strict=True):
        out, err = party.communicate(timeout=300)
        assert party.returncode == 0, err
        assert f"heartbeat slot={slot} generation=1" in out


# --- Newly switched verb families (#2000 cold-review fix) ---

from wrkslots.tests.test_lifecycle import command as _lc_command  # noqa: E402


def test_a_hold_queued_for_the_registry_write_lock_holds_no_config_lock(
    tmp_path: Path,
) -> None:
    project = _project_with_slots(tmp_path)
    _assert_queued_without_config(
        project, ["hold", "slot01", "--reason", "queued hold"], "held slot=slot01"
    )


def test_an_unhold_queued_for_the_registry_write_lock_holds_no_config_lock(
    tmp_path: Path,
) -> None:
    project = _project_with_slots(tmp_path)
    held = _lc_command(project, "hold", "slot01", "--reason", "pre-held")
    assert held.returncode == 0, held.stderr
    _assert_queued_without_config(
        project, ["unhold", "slot01"], "released hold slot=slot01"
    )


def test_a_write_handoff_queued_for_the_registry_write_lock_holds_no_config_lock(
    tmp_path: Path,
) -> None:
    project = _project_with_slots(tmp_path)
    notes = tmp_path / "notes.md"
    notes.write_text("handoff notes\n")
    _assert_queued_without_config(
        project,
        [
            "write-handoff", "slot01", "--agent", "codex-1",
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
            "--from-file", str(notes),
        ],
        "handoff slot=slot01",
    )


def test_a_read_handoff_queued_for_the_registry_write_lock_holds_no_config_lock(
    tmp_path: Path,
) -> None:
    project = _project_with_slots(tmp_path)
    notes = tmp_path / "notes.md"
    notes.write_text("handoff notes\n")
    written = _lc_command(
        project, "write-handoff", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--from-file", str(notes),
    )
    assert written.returncode == 0, written.stderr
    _assert_queued_without_config(
        project,
        ["read-handoff", "slot01", "--coordinator-pid", str(os.getpid())],
        "recorded HANDOFF.md read slot=slot01",
    )


def test_a_release_queued_for_the_registry_write_lock_holds_no_config_lock(
    tmp_path: Path,
) -> None:
    project = _project_with_slots(tmp_path)
    notes = tmp_path / "notes.md"
    notes.write_text("handoff notes\n")
    written = _lc_command(
        project, "write-handoff", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--from-file", str(notes),
    )
    assert written.returncode == 0, written.stderr
    _assert_queued_without_config(
        project,
        [
            "release", "slot01", "--agent", "codex-1",
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        ],
        "released slot=slot01",
    )


# --- Busy-fallback configuration gate (#2000 cold-review fix) ---

import contextlib as _contextlib  # noqa: E402
import json as _json  # noqa: E402
from collections.abc import Iterator as _Iterator  # noqa: E402


def _active_row(project: Path, slot: str) -> dict[str, object]:
    control = control_directory(project)
    for shard in control.glob("ACTIVE.*.json"):
        if shard.name.endswith(".lock"):
            continue
        data = _json.loads(shard.read_text())
        slots: list[dict[str, object]] = data.get("slots", [])
        for row in slots:
            if row.get("slot") == slot:
                return row
    raise AssertionError(f"no row for {slot}")


def _force_busy_then(
    monkeypatch: pytest.MonkeyPatch, project: Path, mutate: str
) -> None:
    """Force the busy fallback, then change one thing while it queues.

    ``mutate == "config"`` appends a comment to the configuration file
    (its sha256 changes, its parsed value does not); ``mutate ==
    "registry"`` holds the other slot (registry churn only). The
    fallback gate is the configuration sha256 alone: config changes
    refuse, registry churn is re-judged fresh by the locked body.
    """
    real_locks = cli._mutation_locks

    def busy(*_args: object, **_kwargs: object) -> None:
        raise cli._RegistryBusy(
            "registry is busy: forced for the fallback test. "
            "state: REFUSED -- nothing was written",
            remedy="retry",
        )

    monkeypatch.setattr(cli, "_run_prevalidated_mutation", busy)
    calls: list[int] = []

    @_contextlib.contextmanager
    def mutating_locks(config: object, wait_seconds: float, **kwargs: object) -> _Iterator[None]:
        if kwargs.get("release_config_after_check") and not calls:
            calls.append(1)
            if mutate == "config":
                config_path = project / ".wrkslots.yml"
                config_path.write_text(
                    config_path.read_text() + "# fallback gate probe\n"
                )
            else:
                changed = _lc_command(project, "hold", "slot02", "--reason", "churn")
                assert changed.returncode == 0, changed.stderr
        with real_locks(config, wait_seconds, **kwargs):  # type: ignore[arg-type]
            yield

    monkeypatch.setattr(cli, "_mutation_locks", mutating_locks)


def test_heartbeat_busy_fallback_refuses_when_config_changes_while_queueing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _project_with_slots(tmp_path)
    before = _active_row(project, "slot01")
    _force_busy_then(monkeypatch, project, "config")
    rc = cli.main(
        [
            "--project-root", str(project), "heartbeat", "slot01",
            "--agent", "codex-1", "--owner-pid", str(os.getpid()),
            "--expected-generation", "1",
        ]
    )
    captured = capsys.readouterr()
    assert rc != 0
    assert "configuration changed while this command was queueing" in captured.err
    assert "nothing was written" in captured.err
    assert _active_row(project, "slot01")["heartbeat_at"] == before["heartbeat_at"]


def test_heartbeat_busy_fallback_tolerates_registry_churn_while_queueing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Other writers' churn is the storm the fallback exists for: it succeeds."""
    project = _project_with_slots(tmp_path)
    before = _active_row(project, "slot01")
    _force_busy_then(monkeypatch, project, "registry")
    rc = cli.main(
        [
            "--project-root", str(project), "heartbeat", "slot01",
            "--agent", "codex-1", "--owner-pid", str(os.getpid()),
            "--expected-generation", "1",
        ]
    )
    captured = capsys.readouterr()
    assert rc == 0, captured.err
    assert _active_row(project, "slot01")["heartbeat_at"] != before["heartbeat_at"]


def test_transfer_busy_fallback_refuses_when_config_changes_while_queueing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _project_with_slots(tmp_path)
    mark_owner_dead(project, slot="slot01")
    before = _active_row(project, "slot01")
    _force_busy_then(monkeypatch, project, "config")
    rc = cli.main(
        [
            "--project-root", str(project), "transfer-owner", "slot01",
            "--agent", "codex-1", "--owner-pid", str(os.getpid()),
            "--expected-generation", "1",
        ]
    )
    captured = capsys.readouterr()
    assert rc != 0
    assert "configuration changed while this command was queueing" in captured.err
    assert "nothing was written" in captured.err
    after = _active_row(project, "slot01")
    assert after["generation"] == before["generation"]
    assert after["agent"] == before["agent"]


def test_transfer_busy_fallback_tolerates_registry_churn_while_queueing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Other writers' churn is the storm the fallback exists for: it succeeds."""
    project = _project_with_slots(tmp_path)
    mark_owner_dead(project, slot="slot01")
    _force_busy_then(monkeypatch, project, "registry")
    rc = cli.main(
        [
            "--project-root", str(project), "transfer-owner", "slot01",
            "--agent", "codex-1", "--owner-pid", str(os.getpid()),
            "--expected-generation", "1",
        ]
    )
    captured = capsys.readouterr()
    assert rc == 0, captured.err
    after = _active_row(project, "slot01")
    owner: dict[str, object] = after["owner"]  # type: ignore[assignment]
    assert owner["pid"] == os.getpid()


def test_a_supersede_queued_for_the_registry_write_lock_holds_no_config_lock(
    tmp_path: Path,
) -> None:
    project = _project_with_slots(tmp_path)
    notes = tmp_path / "notes.md"
    notes.write_text("handoff notes\n")
    written = _lc_command(
        project, "write-handoff", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--from-file", str(notes),
    )
    assert written.returncode == 0, written.stderr
    read = _lc_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert read.returncode == 0, read.stderr
    slot_dir = project / "worktrees" / "slot01"
    assert slot_dir.is_dir(), list((project / "worktrees").iterdir())
    successor = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(120)"],
        cwd=str(slot_dir),
    )
    try:
        _assert_queued_without_config(
            project,
            [
                "supersede-handoff", "slot01", "--agent", "codex-9",
                "--owner-pid", str(successor.pid), "--expected-generation", "1",
                "--coordinator-authorized", "--coordinator-pid", str(os.getpid()),
                "--reason", "queued supersede test",
            ],
            "superseded slot=slot01",
        )
    finally:
        successor.terminate()
        successor.wait(timeout=30)
