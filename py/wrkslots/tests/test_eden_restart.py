"""Per-slot Eden daemon restart checks (#1977 eden-daemon-restart).

A fake ``eden`` executable on PATH logs every invocation and answers
``info``/``start`` from a scenario file, so no real daemon is touched.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli
from wrkslots import eden

REV = "a" * 40
DIGEST = "b" * 64

_FAKE_EDEN = """#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
log = Path(os.environ["FAKE_EDEN_LOG"])
log.parent.mkdir(parents=True, exist_ok=True)
with log.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps(sys.argv[1:]) + "\\n")
scenario = json.loads(Path(os.environ["FAKE_EDEN_SCENARIO"]).read_text())
started_path = Path(os.environ["FAKE_EDEN_STARTED"])
started = set(json.loads(started_path.read_text())) if started_path.exists() else set()
argv = sys.argv[1:]
config_dir = argv[argv.index("--config-dir") + 1]
verb = argv[argv.index("--config-dir") + 2]
mode = scenario.get(config_dir, "healthy")
if verb == "info":
    mount = argv[argv.index("info") + 1]
    if mode == "garbage":
        print("not json")
        raise SystemExit(0)
    if mode == "healthy" or config_dir in started:
        print(json.dumps({
            "mount": mount,
            "state_dir": config_dir + "/client",
            "backing_repo": "/backing/repo",
            "checked_out_revision": "a" * 40,
        }))
        raise SystemExit(0)
    print("daemon is not running", file=sys.stderr)
    raise SystemExit(1)
if verb == "start":
    if mode == "start-fails":
        print("start boom", file=sys.stderr)
        raise SystemExit(1)
    if mode != "still-dead":
        started.add(config_dir)
        started_path.write_text(json.dumps(sorted(started)))
    raise SystemExit(0)
print(f"unexpected verb {verb}", file=sys.stderr)
raise SystemExit(2)
"""


def _project(tmp_path: Path, name: str = "project") -> Path:
    root = tmp_path / name
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "layout: flat\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        f"eden_mount_root: {tmp_path}/eden-mounts\n"
    )
    return root


def _record(root: Path, slot: str) -> wrkslots_cli.ActiveRecord:
    config = wrkslots_cli._load_config(str(root), "node-a")
    slot_path = wrkslots_cli._slot_directory(config, slot, "agent")
    (slot_path / ".eden-state").mkdir(parents=True)
    (slot_path / "checkout").mkdir(parents=True)
    return wrkslots_cli.ActiveRecord(
        slot=slot,
        agent=f"agent-{slot}",
        task="task-1",
        purpose="testing",
        slot_type="agent",
        machine="node-a",
        generation=1,
        created_at="2026-11-20T00:00:00Z",
        heartbeat_at="2026-11-20T00:00:00Z",
        heartbeat_ttl_seconds=60,
        owner=None,
        coordinator_lease=wrkslots_cli._read_process_identity(os.getpid()),
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(
            wrkslots_cli.Checkout(
                name="main",
                path=(slot_path / "checkout").relative_to(config.root).as_posix(),
                repository="/backing/repo",
                branch=eden.EDEN_BRANCH,
                start_point=REV,
                remote=eden.EDEN_REMOTE,
                remote_url_sha256=DIGEST,
                landed_ref=eden.EDEN_LANDED_REF,
                head=REV,
                containing_remote_refs=(),
                vcs="eden",
            ),
        ),
    )


def _write_row(root: Path, record: wrkslots_cli.ActiveRecord) -> None:
    config = wrkslots_cli._load_config(str(root), "node-a")
    wrkslots_cli._ensure_state_shard(config)
    state = wrkslots_cli._load_active(config)
    wrkslots_cli._write_active_state(
        config,
        wrkslots_cli._append_record(state, record),
        action="slot-created",
        slot=record.slot,
        evidence={
            "slot_type": record.slot_type,
            "coordinator_authorized": True,
            "coordinator": wrkslots_cli._identity_to_obj(record.coordinator_lease),
        },
    )


def _config_dir(root: Path, slot: str) -> str:
    config = wrkslots_cli._load_config(str(root), "node-a")
    slot_path = wrkslots_cli._slot_directory(config, slot, "agent")
    return str(slot_path / ".eden-state")


def _install_fake_eden(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, modes: dict[str, str]
) -> Path:
    bin_dir = tmp_path / "fake-bin"
    bin_dir.mkdir(exist_ok=True)
    binary = bin_dir / "eden"
    binary.write_text(_FAKE_EDEN)
    binary.chmod(0o755)
    scenario = tmp_path / "fake-eden-scenario.json"
    scenario.write_text(json.dumps(modes))
    log = tmp_path / "fake-eden-log.jsonl"
    monkeypatch.setenv("FAKE_EDEN_LOG", str(log))
    monkeypatch.setenv("FAKE_EDEN_SCENARIO", str(scenario))
    monkeypatch.setenv("FAKE_EDEN_STARTED", str(tmp_path / "fake-eden-started.json"))
    monkeypatch.setenv("PATH", f"{bin_dir}:{os.environ['PATH']}")
    return log


def _calls(log: Path) -> list[list[str]]:
    if not log.exists():
        return []
    return [json.loads(line) for line in log.read_text().splitlines() if line.strip()]


def _starts(log: Path, config_dir: str) -> int:
    return sum(
        1
        for argv in _calls(log)
        if argv[argv.index("--config-dir") + 1] == config_dir and "start" in argv
    )


def _run(root: Path, *extra: str) -> int:
    return wrkslots_cli.main(
        [
            "--project-root",
            str(root),
            "--machine",
            "node-a",
            "eden",
            "restart-slot-daemons",
            "--format",
            "json",
            *extra,
        ]
    )


def test_dead_daemon_restarted_healthy_skipped_and_idempotent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _project(tmp_path)
    _write_row(root, _record(root, "eden-a"))
    _write_row(root, _record(root, "eden-b"))
    dead, healthy = _config_dir(root, "eden-a"), _config_dir(root, "eden-b")
    log = _install_fake_eden(tmp_path, monkeypatch, {dead: "dead", healthy: "healthy"})

    assert _run(root) == 0
    report = json.loads(capsys.readouterr().out)
    rows = {row["slot"]: row["status"] for row in report["eden_daemons"]}
    assert rows == {"eden-a": "restarted", "eden-b": "already-healthy"}
    assert report["refused"] == 0
    assert _starts(log, dead) == 1
    assert _starts(log, healthy) == 0

    assert _run(root) == 0
    report = json.loads(capsys.readouterr().out)
    rows = {row["slot"]: row["status"] for row in report["eden_daemons"]}
    assert rows == {"eden-a": "already-healthy", "eden-b": "already-healthy"}
    assert _starts(log, dead) == 1, "two runs start each daemon at most once"


def test_slot_limiting_and_unknown_slot_refusal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _project(tmp_path)
    _write_row(root, _record(root, "eden-a"))
    _write_row(root, _record(root, "eden-b"))
    first, second = _config_dir(root, "eden-a"), _config_dir(root, "eden-b")
    log = _install_fake_eden(tmp_path, monkeypatch, {first: "dead", second: "dead"})

    assert _run(root, "--slot", "eden-a") == 0
    report = json.loads(capsys.readouterr().out)
    assert [row["slot"] for row in report["eden_daemons"]] == ["eden-a"]
    assert _starts(log, first) == 1
    assert all(
        argv[argv.index("--config-dir") + 1] != second for argv in _calls(log)
    ), "an unlisted slot is never queried"

    log.write_text("")
    assert _run(root, "--slot", "no-such-slot") == 3
    assert "unknown slot" in capsys.readouterr().err
    assert _calls(log) == [], "an unknown slot refuses before any daemon query"


def test_unqueryable_and_start_failure_refuse_one_slot_and_others_proceed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _project(tmp_path)
    for slot in ("eden-bad", "eden-dead", "eden-fails"):
        _write_row(root, _record(root, slot))
    bad, dead, fails = (
        _config_dir(root, "eden-bad"),
        _config_dir(root, "eden-dead"),
        _config_dir(root, "eden-fails"),
    )
    log = _install_fake_eden(
        tmp_path,
        monkeypatch,
        {bad: "garbage", dead: "dead", fails: "start-fails"},
    )

    assert _run(root) == 3
    report = json.loads(capsys.readouterr().out)
    rows = {row["slot"]: row for row in report["eden_daemons"]}
    assert rows["eden-dead"]["status"] == "restarted"
    assert rows["eden-bad"]["status"] == "refused"
    assert "cannot query" in rows["eden-bad"]["reason"]
    assert rows["eden-fails"]["status"] == "refused"
    assert "start" in rows["eden-fails"]["reason"]
    assert report["refused"] == 2
    assert _starts(log, bad) == 0, "an unqueryable daemon is never started blindly"


def test_second_project_state_is_never_queried(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _project(tmp_path, "project-a")
    other_root = _project(tmp_path, "project-b")
    _write_row(root, _record(root, "eden-a"))
    _write_row(other_root, _record(other_root, "eden-other"))
    own, other = _config_dir(root, "eden-a"), _config_dir(other_root, "eden-other")
    log = _install_fake_eden(tmp_path, monkeypatch, {own: "healthy", other: "dead"})

    assert _run(root) == 0
    report = json.loads(capsys.readouterr().out)
    assert [row["slot"] for row in report["eden_daemons"]] == ["eden-a"]
    assert all(
        argv[argv.index("--config-dir") + 1] != other for argv in _calls(log)
    ), "another project's .eden-state is never queried or started"
    assert _starts(log, other) == 0
