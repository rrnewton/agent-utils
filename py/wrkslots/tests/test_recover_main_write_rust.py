"""Differential: wrkslotsd main recover dispatch outcomes.

#562 rust-primary-wrkslots slice 44, the nineteenth writer slice.
The Python oracle is `_cmd_recover` on a copy of the fixture
registry; the Rust writer runs on the original. Rust covers the
dispatcher's read-only outcomes: coordinator capture, the
flag-combination contract, the no-journal and already-recorded
refusals, and journal schema/machine binding. Resuming a journal,
recovering partial updates, and starting ownerless validation
cleanup stay with the Python edition and refuse explicitly.
Temporary fixture registries only: writers are never pointed at a
live registry before the edition cutover.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import socket
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]
MACHINE = socket.gethostname().split(".", 1)[0]


def _fixture(base: Path) -> Path:
    root = base / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        f"machine: {MACHINE}\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    (root / "worktrees" / f"ACTIVE.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 7, "slots": []})
    )
    (root / "worktrees" / f"ARCHIVED.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 3, "records": []})
    )
    return root


def _run(root: Path, extra: list[str], *, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "recover",
            "--coordinator-pid", str(os.getpid()), "--root", str(root), *extra,
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python(root: Path, **overrides: object) -> int:
    values: dict[str, object] = {
        "project_root": str(root), "machine": None,
        "coordinator_pid": os.getpid(), "slot": None,
        "coordinator_authorized": False, "discard_partial": False,
        "discard_stale_image_copy": False, "abort_import": False,
        "retry_running_hook": False, "abort_create": False,
        "legacy_validate_checkout": None, "ownerless_validate_checkout": None,
        "ownerless_validate_cargo_home": None, "frozen_validate_checkout": None,
        "completed_record": None, "repository": None, "recovery_note": None,
        "validation_proof_manifest": None, "wait_lock": 0.0,
    }
    values.update(overrides)
    return wrkslots_cli._cmd_recover(argparse.Namespace(**values))


def test_recover_no_journal_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    with pytest.raises(Exception) as caught:
        _python(oracle_root)
    assert "no interrupted mutation is recorded" in str(caught.value)
    result = _run(rust_root, [])
    assert result.returncode == 1
    assert "no interrupted mutation is recorded" in result.stderr


def test_recover_flag_combinations_match_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    with pytest.raises(Exception) as caught:
        _python(
            oracle_root, slot="s1",
            ownerless_validate_checkout="ignored/validate/x",
        )
    assert "cannot be combined" in str(caught.value)
    result = _run(
        rust_root,
        ["--slot", "s1", "--ownerless-validate-checkout", "ignored/validate/x"],
    )
    assert result.returncode == 1
    assert "cannot be combined" in result.stderr
    stale = _run(rust_root, ["--discard-stale-image-copy"])
    assert stale.returncode == 1
    assert "--discard-stale-image-copy is the coordinator's decision" in stale.stderr
    legacy = _run(
        rust_root,
        [
            "--legacy-validate-checkout", "ignored/validate/x",
            "--ownerless-validate-cargo-home", "ignored/validate/y",
            "--completed-record", "r.json", "--repository", "../repo",
            "--coordinator-authorized",
        ],
    )
    assert legacy.returncode == 1
    assert "legacy and ownerless validation path flags cannot be combined" in legacy.stderr


def test_recover_ownerless_with_journal_recorded_matches_python(
    tmp_path: Path,
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    for root in (rust_root, oracle_root):
        (root / "worktrees" / f"ACTIVE.{MACHINE}.journal").write_text(
            json.dumps(
                {"schema": 2, "kind": "finish", "machine": MACHINE, "slot": "s1"}
            )
        )
    with pytest.raises(Exception) as caught:
        _python(
            oracle_root, coordinator_authorized=True,
            ownerless_validate_checkout="ignored/validate/x",
            completed_record="r.json", repository="../repo",
        )
    assert "already recorded" in str(caught.value)
    result = _run(
        rust_root,
        [
            "--coordinator-authorized",
            "--ownerless-validate-checkout", "ignored/validate/x",
            "--completed-record", "r.json", "--repository", "../repo",
        ],
    )
    assert result.returncode == 1
    assert "already recorded" in result.stderr


def test_recover_journal_resume_stays_python_owned(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    (rust_root / "worktrees" / f"ACTIVE.{MACHINE}.journal").write_text(
        json.dumps({"schema": 2, "kind": "finish", "machine": MACHINE, "slot": "s1"})
    )
    result = _run(rust_root, [])
    assert result.returncode == 1
    assert "stays with the Python edition" in result.stderr


def test_recover_wrong_machine_journal_refuses(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    (rust_root / "worktrees" / f"ACTIVE.{MACHINE}.journal").write_text(
        json.dumps(
            {"schema": 2, "kind": "finish", "machine": "other-machine", "slot": "s1"}
        )
    )
    result = _run(rust_root, [])
    assert result.returncode == 1
    assert "journal belongs to machine other-machine" in result.stderr


def _scoped_name(operation: str, slot: str) -> str:
    return f"{operation.upper()}.{len(MACHINE)}.{MACHINE}.{len(slot)}.{slot}.journal"


def _events(root: Path, payloads: list[dict[str, object]]) -> None:
    events = root / "worktrees" / f"EVENTS.{MACHINE}"
    events.mkdir(exist_ok=True)
    previous = "0" * 64
    for index, payload in enumerate(payloads, start=1):
        core: dict[str, object] = {
            "schema": 1,
            "machine": MACHINE,
            "sequence": index,
            "previous_sha256": previous,
            "recorded_at": "2026-10-26T03:00:00Z",
            "kind": payload.pop("kind"),
            "payload": payload,
        }
        event = {**core, "sha256": wrkslots_cli._event_digest(core)}
        previous = str(event["sha256"])
        (events / f"{index:020d}.json").write_text(json.dumps(event))


def _journal(operation: str, step: int) -> dict[str, object]:
    base: dict[str, object] = {
        "schema": 2, "kind": operation, "machine": MACHINE, "slot": "s1",
    }
    if operation == "create":
        base.update(
            {"created": [], "hook_progress": step, "operation_id": "a" * 32}
        )
    else:
        base.update(
            {"phase": "prepared" if step == 0 else "fenced", "fenced": "b" * 32}
        )
    return base


def _progress(operation: str, journal: dict[str, object]) -> dict[str, object]:
    return {
        "kind": "operation-progress-recorded",
        "operation": operation,
        "slot": "s1",
        "journal_path": _scoped_name(operation, "s1"),
        "journal": journal,
    }


def _seed_scoped(
    root: Path, operation: str, steps: list[dict[str, object]], on_disk: int
) -> Path:
    name = _scoped_name(operation, "s1")
    path = root / "worktrees" / name
    path.write_text(json.dumps(steps[on_disk]))
    _events(root, [_progress(operation, step) for step in steps])
    return path


def _python_roll_forward(root: Path) -> str:
    config = wrkslots_cli._load_config(str(root), None)
    captured = io.StringIO()
    with contextlib.redirect_stdout(captured):
        wrkslots_cli._roll_forward_lagging_scoped_journals(config)
    return captured.getvalue()


@pytest.mark.parametrize("operation", ["create", "finish"])
def test_recover_rolls_forward_journal_one_step_behind(
    tmp_path: Path, operation: str
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    steps = [_journal(operation, 0), _journal(operation, 1)]
    rust_path = _seed_scoped(rust_root, operation, steps, 0)
    oracle_path = _seed_scoped(oracle_root, operation, steps, 0)
    printed = _python_roll_forward(oracle_root)
    assert "rolled forward" in printed
    assert json.loads(oracle_path.read_text()) == steps[1]
    result = _run(rust_root, [])
    assert result.returncode == 1
    assert "rolled forward scoped " + operation + " journal" in result.stdout
    assert "stays with the Python edition" in result.stderr
    assert json.loads(rust_path.read_text()) == json.loads(oracle_path.read_text())


@pytest.mark.parametrize("operation", ["create", "finish"])
def test_recover_does_not_roll_journal_two_steps_behind(
    tmp_path: Path, operation: str
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    steps = [_journal(operation, step) for step in range(3)]
    # A third distinct progress step so the disk journal is two behind.
    if operation == "create":
        steps[2] = {**steps[1], "hook_progress": 2}
    else:
        steps[2] = {**steps[1], "phase": "archived"}
    rust_path = _seed_scoped(rust_root, operation, steps, 0)
    oracle_path = _seed_scoped(oracle_root, operation, steps, 0)
    before = rust_path.read_bytes()
    assert _python_roll_forward(oracle_root) == ""
    assert oracle_path.read_bytes() == before
    result = _run(rust_root, [])
    assert result.returncode == 1
    assert "rolled forward" not in result.stdout
    assert rust_path.read_bytes() == before


def test_recover_does_not_roll_completed_journal(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    steps = [_journal("create", 0), _journal("create", 1)]
    completion = {
        "kind": "operation-completed",
        "operation": "create",
        "slot": "s1",
        "journal_path": _scoped_name("create", "s1"),
    }
    paths = []
    for root in (rust_root, oracle_root):
        path = _seed_scoped(root, "create", steps, 0)
        _events_append(root, dict(completion))
        paths.append(path)
    rust_path, oracle_path = paths
    before = rust_path.read_bytes()
    assert _python_roll_forward(oracle_root) == ""
    assert oracle_path.read_bytes() == before
    result = _run(rust_root, [])
    assert result.returncode == 1
    assert "rolled forward" not in result.stdout
    assert rust_path.read_bytes() == before


def _events_append(root: Path, payload: dict[str, object]) -> None:
    events = root / "worktrees" / f"EVENTS.{MACHINE}"
    existing = sorted(events.glob("*.json"))
    core_events = [json.loads(path.read_text()) for path in existing]
    index = len(core_events) + 1
    previous = str(core_events[-1]["sha256"]) if core_events else "0" * 64
    core: dict[str, object] = {
        "schema": 1,
        "machine": MACHINE,
        "sequence": index,
        "previous_sha256": previous,
        "recorded_at": "2026-10-26T03:00:00Z",
        "kind": payload.pop("kind"),
        "payload": payload,
    }
    event = {**core, "sha256": wrkslots_cli._event_digest(core)}
    (events / f"{index:020d}.json").write_text(json.dumps(event))


def test_recover_roll_forward_identity_change_refuses_like_python(
    tmp_path: Path,
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    first = _journal("create", 0)
    forged = {**_journal("create", 1), "operation_id": "c" * 32}
    rust_path = _seed_scoped(rust_root, "create", [first, forged], 0)
    oracle_path = _seed_scoped(oracle_root, "create", [first, forged], 0)
    before = rust_path.read_bytes()
    config = wrkslots_cli._load_config(str(oracle_root), None)
    with pytest.raises(Exception) as caught:
        wrkslots_cli._roll_forward_lagging_scoped_journals(config)
    assert "progress changed its operation identity" in str(caught.value)
    assert oracle_path.read_bytes() == before
    result = _run(rust_root, [])
    assert result.returncode == 1
    assert "progress changed its operation identity" in result.stderr
    assert rust_path.read_bytes() == before


def test_recover_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    result = _run(rust_root, [], fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
