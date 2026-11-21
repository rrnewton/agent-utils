"""Directory-fsync fault at the rename boundary (#2049).

Recreated from mu-slotmodel's #2043 fault harness with the pinned
contract flipped to the fixed behaviour. When the directory fsync
after ``os.replace`` fails, the rename has already landed, so the
outcome is reported as a classified refusal saying the state may
be committed and naming ``wrkslots recover``; it is never an
unclassified traceback and never retried (a failed fsync can clear
the error state, so a retry returning 0 would not prove
durability). Before the rename, the refusal stays consistent:
nothing is durable. The reverse mismatch (reported success,
nothing durable) must never occur.
"""

from __future__ import annotations

import os
import stat
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import e2e_stress as e2e  # type: ignore[import-not-found]  # noqa: E402

from wrkslots import cli, slotimage  # noqa: E402

_FAULT_PROGRAM = """
import os, runpy, stat, sys
real_fsync = os.fsync
target = int(os.environ.get("WRKSLOTS_FUZZ_FAULT_AFTER", "0"))
seen = {"n": 0}
def flaky(fd):
    try:
        is_dir = stat.S_ISDIR(os.fstat(fd).st_mode)
    except OSError:
        is_dir = False
    if is_dir:
        if seen["n"] == target:
            raise OSError(5, "injected directory fsync failure")
        seen["n"] += 1
    return real_fsync(fd)
os.fsync = flaky
sys.argv = ["wrkslots"] + sys.argv[1:]
runpy.run_module("wrkslots", run_name="__main__")
"""

_DECODE_PROGRAM = """
import json, sys
from wrkslots import cli
root, machine = sys.argv[1], sys.argv[2]
config = cli._load_config(root, machine)
out = {"chain_loads": False, "slots": [], "journals": []}
try:
    states = cli._load_all_active(config, require_repository=False)
    out["chain_loads"] = True
    state = next(s for s in states if s.machine == machine)
    out["slots"] = sorted(r.slot for r in state.slots)
except Exception as exc:
    out["error"] = f"{type(exc).__name__}: {exc}"
for pat in ("FINISH.*.journal", "CREATE.*.journal", "ACTIVE.*.journal"):
    out["journals"] += sorted(p.name for p in config.control.glob(pat))
print(json.dumps(out, sort_keys=True))
"""


def _cli_fault(
    project: e2e.TestProject, *args: str, after: int
) -> subprocess.CompletedProcess[str]:
    env = dict(project.environment)
    env["WRKSLOTS_FUZZ_FAULT_AFTER"] = str(after)
    return subprocess.run(
        [sys.executable, "-c", _FAULT_PROGRAM, "--project-root",
         str(project.root), "--machine", e2e.MACHINE, "--wait-lock", "10",
         *args],
        cwd=project.root, env=env, text=True, capture_output=True,
        timeout=120, check=False,
    )


def _create(
    project: e2e.TestProject, *, after: int
) -> subprocess.CompletedProcess[str]:
    return _cli_fault(
        project, "create", "fz-a", "--slot-type", "agent",
        "--coordinator-authorized", "--agent", "fz-fz-a",
        "--task", "task-fz-a", "--purpose", "fault fsync",
        "--coordinator-pid", str(os.getpid()),
        "--repo", "product=product", "--branch", "product=e2e/fz-a",
        after=after,
    )


def _decode(project: e2e.TestProject) -> dict[str, object]:
    import json

    done = subprocess.run(
        [sys.executable, "-c", _DECODE_PROGRAM, str(project.root), e2e.MACHINE],
        cwd=project.root, env=project.environment, text=True,
        capture_output=True, timeout=120, check=False,
    )
    assert done.returncode == 0, done.stderr
    value: dict[str, object] = json.loads(done.stdout)
    return value


def test_fsync_before_any_durable_write_refuses_consistently(
    tmp_path: Path,
) -> None:
    trace = e2e.Trace(tmp_path / "trace.jsonl", seed=100)
    project = e2e._make_project(tmp_path, trace, ttl_seconds=2)
    done = _create(project, after=0)
    assert done.returncode == 3
    assert "Traceback" not in done.stderr
    state = _decode(project)
    assert state["slots"] == [] and state["journals"] == []


def test_fsync_at_journal_boundary_reports_may_be_committed(
    tmp_path: Path,
) -> None:
    trace = e2e.Trace(tmp_path / "trace.jsonl", seed=103)
    project = e2e._make_project(tmp_path, trace, ttl_seconds=2)
    done = _create(project, after=3)
    assert done.returncode == 3
    assert "may be committed" in done.stderr
    assert "wrkslots recover" in done.stderr
    assert "cannot durably replace state file" not in done.stderr
    assert "Traceback" not in done.stderr
    state = _decode(project)
    assert state["journals"]  # the CREATE journal is durable
    # One recover completes the interrupted create.
    e2e._cli(project, "recover", "--coordinator-pid", str(os.getpid()),
             trace=trace)
    healed = _decode(project)
    assert healed["chain_loads"] is True
    assert healed["slots"] == ["fz-a"]
    assert healed["journals"] == []


def test_fsync_sweep_never_reports_success_without_durable_state(
    tmp_path: Path,
) -> None:
    # Reverse direction: a reported success must always be durable.
    for after in range(6):
        trace = e2e.Trace(tmp_path / f"trace{after}.jsonl", seed=110 + after)
        project = e2e._make_project(tmp_path / f"p{after}", trace, ttl_seconds=2)
        done = _create(project, after=after)
        assert "Traceback" not in done.stderr
        if done.returncode == 0:
            slots = _decode(project)["slots"]
            assert isinstance(slots, list) and "fz-a" in slots


def _fail_dir_fsync(monkeypatch: pytest.MonkeyPatch) -> None:
    real_fsync = os.fsync

    def flaky(fd: int) -> None:
        if stat.S_ISDIR(os.fstat(fd).st_mode):
            raise OSError(5, "injected directory fsync failure")
        real_fsync(fd)

    monkeypatch.setattr(os, "fsync", flaky)


def test_atomic_write_dir_fsync_after_rename_is_may_be_committed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "state.json"
    _fail_dir_fsync(monkeypatch)
    with pytest.raises(cli.Refusal, match="may be committed"):
        cli._atomic_write_bytes(target, b"{}\n")
    # The rename landed even though durability is unknown.
    assert target.read_bytes() == b"{}\n"


def test_atomic_write_file_fsync_failure_refuses_consistently(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "state.json"

    def flaky(fd: int) -> None:
        raise OSError(5, "injected fsync failure")

    monkeypatch.setattr(os, "fsync", flaky)
    with pytest.raises(cli.Refusal, match="cannot durably replace"):
        cli._atomic_write_bytes(target, b"{}\n")
    assert not target.exists()


def test_write_record_dir_fsync_after_rename_is_may_be_committed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    directory = tmp_path / "agent" / "fz-a"
    directory.mkdir(parents=True)
    image = slotimage.SlotImage(
        directory=directory, slot="fz-a", slot_type="agent",
        location=tmp_path / "slots" / "fz-a", ceiling_bytes=1024,
        state_ceiling_bytes=1024, backend="kernel",
        created_at="2026-01-01T00:00:00+00:00", phase="provisioning",
    )
    _fail_dir_fsync(monkeypatch)
    with pytest.raises(slotimage.ImageError, match="may be committed"):
        slotimage._write_record(image)
    assert image.record_path.exists()
