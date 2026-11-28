"""A live owner with an expired heartbeat is named, not just labelled (#2466).

`owner=live` comes from the recorded process generation (PID, start ticks,
boot and host identity); heartbeat age never enters it. When the two
disagree, status and doctor name the live process so an operator can tell
a leftover from a real owner. Display only: no verdict changes.
"""

from __future__ import annotations

import datetime as dt
import json
import os
import subprocess
import sys
from dataclasses import replace
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import create, expire_heartbeat, make_project


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _age_heartbeat(project: Path, seconds: int) -> None:
    """Record the first row's last heartbeat ``seconds`` ago."""
    config = cli._load_config(str(project), "testhost")
    state = cli._load_active(config)
    record = state.slots[0]
    heartbeat_at = (
        dt.datetime.now(dt.timezone.utc) - dt.timedelta(seconds=seconds)
    ).isoformat(timespec="seconds")
    cli._write_active_state(
        config,
        cli._replace_record(state, replace(record, heartbeat_at=heartbeat_at)),
        action="test-heartbeat-age",
        slot=record.slot,
    )


def _past_threshold(project: Path) -> int:
    config = cli._load_config(str(project), "testhost")
    ttl = cli._load_active(config).slots[0].heartbeat_ttl_seconds
    return cli._owner_live_expired_finding_threshold(ttl) + 60


def _json(project: Path, command: str, capsys: pytest.CaptureFixture[str]) -> dict[str, object]:
    code = cli.main(["--project-root", str(project), command, "--format", "json"])
    payload = json.loads(capsys.readouterr().out)
    assert code == 0, payload
    assert isinstance(payload, dict)
    return payload


def _rows(payload: dict[str, object]) -> list[dict[str, object]]:
    rows = payload["active"]
    assert isinstance(rows, list)
    return rows


def _findings(payload: dict[str, object], kind: str) -> list[dict[str, object]]:
    findings = payload["findings"]
    assert isinstance(findings, list)
    return [item for item in findings if item["kind"] == kind]


def test_a_live_owner_with_an_expired_heartbeat_is_named(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    # The owner is this test process: live for the whole test.
    assert create(project).returncode == 0
    status = _json(project, "status", capsys)
    (row,) = _rows(status)
    assert row["owner_state"] == "live" and row["heartbeat_expired"] is False
    assert "owner_process" not in row
    assert _findings(_json(project, "doctor", capsys), "owner-live-heartbeat-expired") == []

    expire_heartbeat(project)
    status = _json(project, "status", capsys)
    (row,) = _rows(status)
    assert row["owner_state"] == "live" and row["heartbeat_expired"] is True
    process = row["owner_process"]
    assert isinstance(process, dict)
    assert process["pid"] == os.getpid()
    assert process["comm"] == Path(f"/proc/{os.getpid()}/comm").read_text().strip()
    assert isinstance(process["argv"], str) and "python" in process["argv"]
    assert str(process["cgroup"]).startswith("/")
    started = str(process["started_at"])
    assert started.endswith("Z") and started[:4].isdigit()

    # A heartbeat that has only just lapsed (one long turn) is shown on the
    # row but reported by no doctor finding.
    doctor = _json(project, "doctor", capsys)
    assert _findings(doctor, "owner-live-heartbeat-expired") == []
    (doctor_row,) = _rows(doctor)
    assert doctor_row["owner_process"] == process

    # Past the threshold: exactly one finding, keyed by the slot, the same
    # on every run.
    _age_heartbeat(project, _past_threshold(project))
    keys = []
    for _run in range(2):
        doctor = _json(project, "doctor", capsys)
        (finding,) = _findings(doctor, "owner-live-heartbeat-expired")
        assert finding["slot"] == "slot01"
        detail = str(finding["detail"])
        assert f"PID {os.getpid()}" in detail
        assert str(process["comm"]) in detail and str(process["started_at"]) in detail
        # The command line stays out of the detail (consumers relay details
        # into posts); the row's owner_process keeps it.
        assert "command line" not in detail
        assert str(process["argv"]) not in detail
        assert "by its PID" in str(finding["remedy"])
        keys.append((finding["kind"], finding["slot"], finding["detail"]))
    assert keys[0] == keys[1]
    quick = cli.main(["--project-root", str(project), "doctor", "--quick", "--format", "json"])
    quick_payload = json.loads(capsys.readouterr().out)
    assert quick == 0
    assert len(_findings(quick_payload, "owner-live-heartbeat-expired")) == 1

    human = cli.main(["--project-root", str(project), "status"])
    out = capsys.readouterr().out
    assert human == 0
    assert f"owner process: pid={os.getpid()} " in out


def test_the_finding_threshold_is_one_more_ttl_and_at_least_an_hour() -> None:
    assert cli._owner_live_expired_finding_threshold(3600) == 7200
    assert cli._owner_live_expired_finding_threshold(60) == 60 + 3600
    assert cli._owner_live_expired_finding_threshold(7200) == 14400



def test_a_dead_owner_with_an_expired_heartbeat_is_not_named(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(600)"])
    try:
        assert create(project, owner_pid=child.pid).returncode == 0
    finally:
        child.kill()
        child.wait()
    expire_heartbeat(project)
    (row,) = _rows(_json(project, "status", capsys))
    assert row["owner_state"] == "dead"
    assert "owner_process" not in row
    doctor = _json(project, "doctor", capsys)
    assert _findings(doctor, "owner-live-heartbeat-expired") == []
    assert len(_findings(doctor, "dead-owner")) == 1


def test_rust_names_the_same_live_owner(tmp_path: Path) -> None:
    """Status and doctor in the Rust edition carry the same field and finding (#2466)."""
    from wrkslots.tests.rust_binary import rust_wrkslotsd

    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    _age_heartbeat(project, _past_threshold(project))
    env = dict(os.environ)
    env["PYTHONPATH"] = str(Path(__file__).resolve().parents[2])

    def normalized(text: str) -> dict[str, object]:
        document = json.loads(text)
        assert isinstance(document, dict)
        document.pop("timings", None)
        document.pop("subprocesses", None)
        for row in document["active"]:
            row.pop("heartbeat_age_seconds", None)
        return document

    for python_args, rust_args in (
        (["status", "--format", "json"], ["status-text", "--root", str(project), "--format", "json"]),
        (
            ["doctor", "--quick", "--format", "json"],
            ["doctor-text", "--quick", "--root", str(project), "--format", "json"],
        ),
    ):
        python_result = subprocess.run(
            [sys.executable, "-m", "wrkslots", "--project-root", str(project), *python_args],
            capture_output=True, text=True, check=False, env=env, timeout=900,
        )
        rust_result = subprocess.run(
            [str(rust_wrkslotsd()), *rust_args],
            capture_output=True, text=True, check=False, timeout=900,
        )
        assert python_result.returncode == 0, python_result.stderr
        assert rust_result.returncode == 0, rust_result.stderr
        python_document = normalized(python_result.stdout)
        rust_document = normalized(rust_result.stdout)
        if python_args[0] == "status":
            assert rust_document == python_document
        else:
            # Quick mode's liveness columns differ between the editions
            # already (Python skips the probe); the findings must agree.
            assert rust_document["findings"] == python_document["findings"]
            assert len(_findings(python_document, "owner-live-heartbeat-expired")) == 1
        rows = python_document["active"]
        assert isinstance(rows, list) and "owner_process" in rows[0]
        rust_rows = rust_document["active"]
        assert isinstance(rust_rows, list)
        assert rust_rows[0]["owner_process"] == rows[0]["owner_process"]
