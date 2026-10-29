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
import json
import os
import socket
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


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
            "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
            "-p", "wrkslotsd", "--", "recover",
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


def test_recover_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    result = _run(rust_root, [], fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
