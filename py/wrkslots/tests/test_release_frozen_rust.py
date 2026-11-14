"""Differential: wrkslotsd release-frozen-incomplete dry run (#1089).

#562 rust-primary-wrkslots step 4a. The Python oracle is
`_cmd_release_frozen_incomplete` on a copy of the current-incomplete
frozen fixture; the Rust command runs on the original. The dry-run
payload must agree; the `--yes` execution stays with the Python
edition and refuses explicitly in Rust. Fixture registries only.
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
REASON = "the result was lost to a fixed parser defect"


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(cwd), *args],
        capture_output=True, text=True, check=False,
    )
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def _run(command: list[str]) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    return subprocess.run(
        [str(rust_wrkslotsd()), *command],
        cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=900, env=env,
    )


def _fixture(base: Path) -> tuple[Path, Path]:
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
    remote = base / "remote.git"
    _git(base, "init", "-q", "--bare", str(remote))
    upstream = root / "upstream-repo"
    upstream.mkdir(parents=True)
    _git(upstream, "init", "-q", "-b", "main")
    (upstream / ".gitignore").write_text("ignored/\n")
    schema = upstream / "ci" / "manifest-plan" / "validation-service-result-schema.json"
    schema.parent.mkdir(parents=True)
    schema.write_text(json.dumps({
        "schema_version": 5,
        "fields": [
            "schema_version", "commit", "profile", "selection_mode",
            "final_validate_status", "detail", "exit_code", "executed_nodes",
            "executed_tests", "passed_tests", "scorecard_writeback",
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
    _git(upstream, "add", ".")
    _git(upstream, "-c", "user.email=test@example.invalid", "-c",
         "user.name=Test", "commit", "-q", "-m", "one")
    _git(upstream, "remote", "add", "origin", str(remote))
    _git(upstream, "push", "-q", "origin", "main")
    _git(upstream, "-c", "user.email=test@example.invalid", "-c",
         "user.name=Test", "commit", "-q", "--allow-empty", "-m", "local-only target")
    target = _git(upstream, "rev-parse", "HEAD")
    frozen_root = root.parent / f".{root.name}-frozen-validate"
    frozen_root.mkdir(mode=0o700)
    checkout = frozen_root / "validate-fresh-a"
    _git(root, "clone", "-q", "--no-checkout", "--no-tags",
         str(upstream), str(checkout))
    _git(checkout, "remote", "set-url", "origin", str(remote))
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
        "admission_result": {
            "state": "admitted",
            "recorded_at": "2026-09-08T11:28:07+00:00",
            "run_number": 1,
        },
        "service_result_schema": 5,
        "result_source": "validation-service-result",
        "materialized_target": False,
        "qualifying_receipt": False,
        "state": "completed",
        "result": "no-result",
        "final_validate_status": "COULD_NOT_RUN",
        "exit_code": 75,
        "executed_nodes": 0,
        "executed_tests": None,
        "passed_tests": None,
        "selection_mode": None,
        "scorecard_writeback": None,
        "detail": "producer could not prepare validation",
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
        "schema_version": 5,
        "commit": target,
        "profile": "only-full",
        "selection_mode": None,
        "detail": ["producer could not prepare validation"],
        "final_validate_status": "COULD_NOT_RUN",
        "exit_code": 75,
        "executed_nodes": 0,
        "executed_tests": None,
        "passed_tests": None,
        "scorecard_writeback": None,
    }))
    return root, checkout


def _normalized(payload: dict[str, object], root: Path, checkout: Path) -> dict[str, object]:
    payload = dict(payload)
    assert isinstance(payload["bytes"], int) and payload["bytes"] > 0
    payload["bytes"] = 0
    payload["checkout"] = "CHECKOUT"
    payload["target"] = "TARGET"
    payload["record_sha256"] = "SHA"
    payload["repositories"] = [
        {**row, "head": "TARGET"}
        for row in payload["repositories"]  # type: ignore[attr-defined]
    ]
    return payload


def test_release_dry_run_matches_python(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    rust_root, rust_checkout = _fixture(tmp_path / "rust")
    oracle_root, oracle_checkout = _fixture(tmp_path / "oracle")

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
        wait_lock=0.0,
        frozen_validate_checkout=str(oracle_checkout),
        completed_record="ignored/validate/runs/run-frozen.json",
        repository="upstream-repo",
        reason=REASON,
        yes=False,
        coordinator_authorized=True,
        coordinator_pid=os.getpid(),
        format="json",
    )
    assert wrkslots_cli._cmd_release_frozen_incomplete(args) == 0
    expected = json.loads(capsys.readouterr().out)
    result = _run(
        [
            "release-frozen-incomplete",
            "--coordinator-authorized", "--coordinator-pid", str(os.getpid()),
            "--frozen-validate-checkout", str(rust_checkout),
            "--completed-record", "ignored/validate/runs/run-frozen.json",
            "--repository", "upstream-repo",
            "--reason", REASON,
            "--format", "json",
            "--root", str(rust_root),
        ]
    )
    assert result.returncode == 0, result.stderr
    mine = json.loads(result.stdout)
    assert _normalized(mine, rust_root, rust_checkout) == _normalized(
        expected, oracle_root, oracle_checkout
    )
    assert rust_checkout.is_dir()


def test_release_yes_execution_stays_python_owned(tmp_path: Path) -> None:
    root, checkout = _fixture(tmp_path / "rust")
    result = _run(
        [
            "release-frozen-incomplete",
            "--coordinator-authorized", "--coordinator-pid", str(os.getpid()),
            "--frozen-validate-checkout", str(checkout),
            "--completed-record", "ignored/validate/runs/run-frozen.json",
            "--repository", "upstream-repo",
            "--reason", REASON,
            "--yes",
            "--root", str(root),
        ]
    )
    assert result.returncode == 1
    assert "stays with the Python edition" in result.stderr
    assert checkout.is_dir()
