"""Differential: wrkslotsd ownerless cache and validate-batch plans.

#562 rust-primary-wrkslots slice 43, the eighteenth writer slice.
The Python oracles are `_cmd_recover_ownerless_agent_cache`,
`_cmd_recover_ownerless_validate_batch`, and
`_cmd_classify_ownerless_validate_batch` on copies of the fixture
registry; the Rust writer runs on the original. Rust covers the
cache plan outcomes (journal resume, registry collision,
destination-exists, exact cache tree) and the validate-batch
argument-shape contract; the relocation, the shared censuses, and
the batch recovery stay with the Python edition and refuse
explicitly. Temporary fixture registries only: writers are never
pointed at a live registry before the edition cutover.
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

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]
MACHINE = socket.gethostname().split(".", 1)[0]


def _fixture(base: Path, *, cache: bool = True) -> Path:
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
    if cache:
        script = root / "worktrees" / "ignored" / "validate" / "cache" / "rust-script"
        script.mkdir(parents=True)
        (script / "blob.bin").write_bytes(b"cached")
    (root / "worktrees" / f"ACTIVE.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 7, "slots": []})
    )
    (root / "worktrees" / f"ARCHIVED.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 3, "records": []})
    )
    return root


def _run(command: list[str], *, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [
            str(rust_wrkslotsd()), *command,
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python_cache(root: Path) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, apply=False,
        coordinator_authorized=False, coordinator_pid=None, wait_lock=0.0,
    )
    return wrkslots_cli._cmd_recover_ownerless_agent_cache(args)


@pytest.mark.load_quarantine  # #1267 wrkslots-load-robust
def test_cache_plan_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    assert _python_cache(oracle_root) == 0
    expected = capsys.readouterr().out.strip()
    result = _run(["recover-ownerless-agent-cache", "--root", str(rust_root)])
    assert result.returncode == 0, result.stderr
    outcome = json.loads(result.stdout)
    assert outcome["outcome"] == "planned"
    assert outcome["detail"] == str(
        rust_root / "ignored" / "validate" / "cache" / "wrkslots-agent-ignored"
    )
    assert expected == (
        "CACHE slot=ignored outcome=planned destination="
        f"{oracle_root / 'ignored' / 'validate' / 'cache' / 'wrkslots-agent-ignored'}"
    )


def test_cache_journal_resume_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    for root in (rust_root, oracle_root):
        source = root / "worktrees" / "ignored"
        journal = {
            "schema": 2,
            "kind": "ownerless-agent-cache-relocate",
            "machine": MACHINE,
            "host_id": wrkslots_cli._host_id(),
            "slot": "ignored",
            "phase": "prepared",
            "source": "worktrees/ignored",
            "destination": "ignored/validate/cache/wrkslots-agent-ignored",
            "fenced": "worktrees/.ignored.ownerless-agent-cache." + "0" * 32,
            "identity": list(
                wrkslots_cli._open_directory_identity(source, "ownerless agent cache")
            ),
            "actor": wrkslots_cli._identity_to_obj(
                wrkslots_cli._read_process_identity(os.getpid())
            ),
        }
        (root / "worktrees" / f"ACTIVE.{MACHINE}.journal").write_text(
            json.dumps(journal)
        )
    assert _python_cache(oracle_root) == 0
    expected = capsys.readouterr().out.strip()
    result = _run(["recover-ownerless-agent-cache", "--root", str(rust_root)])
    assert result.returncode == 0, result.stderr
    outcome = json.loads(result.stdout)
    assert outcome["outcome"] == "planned"
    assert outcome["detail"] == "resume"
    assert expected == "CACHE slot=ignored outcome=planned detail=resume"


def test_cache_missing_tree_refuses(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust", cache=False)
    oracle_root = _fixture(tmp_path / "oracle", cache=False)
    with pytest.raises(Exception) as caught:
        _python_cache(oracle_root)
    assert "absent or unsafe" in str(caught.value)
    result = _run(["recover-ownerless-agent-cache", "--root", str(rust_root)])
    assert result.returncode == 1
    assert "absent or unsafe" in result.stderr


def test_cache_unexpected_entry_refuses(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "ignored" / "stray.txt").write_text("x\n")
    with pytest.raises(Exception) as caught:
        _python_cache(oracle_root)
    assert "unexpected entry" in str(caught.value)
    result = _run(["recover-ownerless-agent-cache", "--root", str(rust_root)])
    assert result.returncode == 1
    assert "unexpected entry" in result.stderr


def test_cache_destination_exists_refuses(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    for root in (rust_root, oracle_root):
        destination = root / "ignored" / "validate" / "cache" / "wrkslots-agent-ignored"
        destination.mkdir(parents=True)
    with pytest.raises(Exception) as caught:
        _python_cache(oracle_root)
    assert "destination already exists" in str(caught.value)
    result = _run(["recover-ownerless-agent-cache", "--root", str(rust_root)])
    assert result.returncode == 1
    assert "destination already exists" in result.stderr


def test_cache_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    result = _run(
        ["recover-ownerless-agent-cache", "--root", str(rust_root)], fixture=False
    )
    assert result.returncode == 1
    assert "fixture-only" in result.stderr


def test_batch_shape_refusals_match_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "rust")
    both = _run(
        [
            "recover-ownerless-validate-batch",
            "--checkout", "worktrees/validate/a",
            "--frozen-validate-checkout", "/tmp/frozen/a",
            "--completed-record", "record.json",
            "--repository", "../repo",
            "--coordinator-authorized", "--coordinator-pid", str(os.getpid()),
            "--root", str(root),
        ]
    )
    assert both.returncode == 1
    assert "exactly one of --checkout or --frozen-validate-checkout" in both.stderr
    misaligned = _run(
        [
            "recover-ownerless-validate-batch",
            "--checkout", "worktrees/validate/a",
            "--completed-record", "record.json",
            "--coordinator-authorized", "--coordinator-pid", str(os.getpid()),
            "--root", str(root),
        ]
    )
    assert misaligned.returncode == 1
    assert "same number of" in misaligned.stderr
    duplicate = _run(
        [
            "classify-ownerless-validate-batch",
            "--checkout", "worktrees/validate/a",
            "--checkout", "worktrees/validate/a",
            "--completed-record", "r1", "--completed-record", "r2",
            "--repository", "../a", "--repository", "../b",
            "--root", str(root),
        ]
    )
    assert duplicate.returncode == 1
    assert "duplicate checkout" in duplicate.stderr


def test_batch_recovery_stays_python_owned(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "rust")
    result = _run(
        [
            "recover-ownerless-validate-batch",
            "--checkout", "worktrees/validate/a",
            "--completed-record", "record.json",
            "--repository", "../repo",
            "--coordinator-authorized", "--coordinator-pid", str(os.getpid()),
            "--root", str(root),
        ]
    )
    assert result.returncode == 1
    assert "stays with the Python edition" in result.stderr


def test_batch_classify_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    args = argparse.Namespace(
        project_root=str(oracle_root),
        machine=None,
        checkouts=["worktrees/validate/a"],
        frozen_validate_checkouts=[],
        completed_records=["record.json"],
        repositories=["../repo"],
        validation_proof_manifests=[],
        format="json",
    )
    assert wrkslots_cli._cmd_classify_ownerless_validate_batch(args) == 0
    expected = capsys.readouterr().out.strip().replace(
        str(oracle_root), str(rust_root)
    )
    classify = _run(
        [
            "classify-ownerless-validate-batch",
            "--checkout", "worktrees/validate/a",
            "--completed-record", "record.json",
            "--repository", "../repo",
            "--format", "json",
            "--root", str(rust_root),
        ]
    )
    assert classify.returncode == 0, classify.stderr
    assert json.loads(classify.stdout) == json.loads(expected)


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(cwd), *args],
        capture_output=True, text=True, check=False,
    )
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def _frozen_fixture(base: Path) -> tuple[Path, Path, str]:
    root = _fixture(base)
    upstream = root / "upstream-repo"
    upstream.mkdir()
    schema = upstream / "ci" / "manifest-plan" / "validation-service-result-schema.json"
    schema.parent.mkdir(parents=True)
    schema.write_text(json.dumps({
        "schema_version": 2,
        "fields": [
            "schema_version", "commit", "profile", "final_validate_status",
            "exit_code", "executed_nodes", "executed_tests", "scorecard_writeback",
        ],
        "outcomes": {
            "PASSED": {"validation_exit_code": 0, "state": "completed", "result": "success"},
            "FAILED": {"validation_exit_code": 1, "state": "completed", "result": "failure"},
            "COULD_NOT_RUN": {"validation_exit_code": 75, "state": "completed", "result": "no-result"},
        },
        "scorecard_writeback": {
            "nullable": True,
            "variants": {"completed": ["status"], "failed": ["status", "error"]},
        },
    }))
    (upstream / "file.txt").write_text("one\n")
    _git(upstream, "init", "-q", "-b", "main")
    _git(upstream, "add", ".")
    _git(upstream, "-c", "user.email=test@example.invalid", "-c",
         "user.name=Test", "commit", "-q", "-m", "one")
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    target = _git(upstream, "rev-parse", "HEAD")
    frozen_root = root.parent / f".{root.name}-frozen-validate"
    frozen_root.mkdir(mode=0o700)
    checkout = frozen_root / "validate-fresh-a"
    _git(root, "clone", "-q", "--no-checkout", "--no-tags",
         str(upstream), str(checkout))
    _git(checkout, "checkout", "-q", "--detach", target)
    checkout.chmod(0o700)
    runs = root / "ignored" / "validate" / "runs"
    runs.mkdir(parents=True)
    record = {
        "schema_version": 1,
        "kind": "frozen-validate",
        "producer": "ci-hub/validate/run_registry.py",
        "admission": "frozen-validate",
        "validation_kind": "frozen-validate",
        "service_result_schema": 2,
        "result_source": "historical-validation-service-result",
        "materialized_target": False,
        "qualifying_receipt": False,
        "state": "unknown",
        "result": "unknown",
        "final_validate_status": "FAILED",
        "exit_code": 1,
        "executed_nodes": 14,
        "executed_tests": None,
        "passed_tests": None,
        "detail": (
            "validation-service-result-schema_version: historical schema 2 "
            "is readable but has no current authority"
        ),
        "scorecard_writeback": {"status": "completed"},
        "process_identity": {
            "pid": 999999, "start_ticks": 1, "boot_id": "historical-boot",
        },
        "checkout": str(checkout),
        "source_checkout": str(upstream),
        "target": target,
        "repo": "consumer-a/project",
        "temporary_checkout": True,
    }
    (runs / "run-frozen.json").write_text(json.dumps(record))
    (runs / "run-frozen.service-result.json").write_text(json.dumps({
        "schema_version": 2,
        "commit": target,
        "profile": "only-portable",
        "final_validate_status": "FAILED",
        "exit_code": 1,
        "executed_nodes": 14,
        "executed_tests": None,
        "scorecard_writeback": {"status": "completed"},
    }))
    return root, checkout, target


def test_batch_classify_frozen_historical_matches_python(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    rust_root, rust_checkout, _ = _frozen_fixture(tmp_path / "rust")
    oracle_root, oracle_checkout, _ = _frozen_fixture(tmp_path / "oracle")

    def canonical(_config: object, _path: Path, contents: bytes) -> tuple[object, object]:
        parsed = wrkslots_cli._strict_json_object(contents, "frozen fixture")
        return (
            {
                "schema_version": parsed["schema_version"],
                "kind": parsed["kind"],
                "state": parsed["state"],
                "target": parsed["target"],
                "repo": parsed["repo"],
            },
            parsed,
        )

    empty = lambda _paths, **_kwargs: wrkslots_cli._ProcessPathCensus((), ())
    monkeypatch.setattr(wrkslots_cli, "_canonical_frozen_validation_record", canonical)
    monkeypatch.setattr(wrkslots_cli, "_capture_lsof_process_path_census", empty)
    monkeypatch.setattr(wrkslots_cli, "_capture_same_uid_process_path_census", empty)
    monkeypatch.setattr(wrkslots_cli, "_capture_process_path_census", empty)
    args = argparse.Namespace(
        project_root=str(oracle_root),
        machine=None,
        checkouts=[],
        frozen_validate_checkouts=[str(oracle_checkout)],
        completed_records=["ignored/validate/runs/run-frozen.json"],
        repositories=["upstream-repo"],
        validation_proof_manifests=[],
        format="json",
    )
    assert wrkslots_cli._cmd_classify_ownerless_validate_batch(args) == 0
    expected = capsys.readouterr().out.strip().replace(
        str(oracle_root), str(rust_root)
    ).replace(str(oracle_checkout), str(rust_checkout))
    classify = _run(
        [
            "classify-ownerless-validate-batch",
            "--frozen-validate-checkout", str(rust_checkout),
            "--completed-record", "ignored/validate/runs/run-frozen.json",
            "--repository", "upstream-repo",
            "--format", "json",
            "--root", str(rust_root),
        ]
    )
    assert classify.returncode == 0, classify.stderr
    assert json.loads(classify.stdout) == json.loads(expected)
