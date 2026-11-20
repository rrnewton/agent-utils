"""#1963: agentctl redact in the Python edition."""
from __future__ import annotations

import json
import stat
from pathlib import Path

import pytest

from agentctl.errors import AgentDeliveryError
from .test_herdr_subagents import setup

SECRET = "SECRET-123456"


def _stop(manager: object) -> None:
    record = manager.get("worker")  # type: ignore[attr-defined]
    record.lifecycle = "stopped"
    manager._save(record)  # type: ignore[attr-defined]


def _seed(manager: object) -> Path:
    directory = Path(manager._directory("worker"))  # type: ignore[attr-defined]
    processed = directory / "queue" / "processed"
    processed.mkdir(parents=True, exist_ok=True)
    (processed / "m1.json").write_text(
        json.dumps({"text": f"the {SECRET} and {SECRET} again", "count": 42, SECRET: "key name stays"}),
        encoding="utf-8",
    )
    events = directory / "events"
    events.mkdir(parents=True, exist_ok=True)
    (events / "e.jsonl").write_text(f'{{"note":"a {SECRET} here"}}\nnot json {SECRET}\n', encoding="utf-8")
    return directory


def test_redact_rewrites_only_string_values_with_backups_and_audit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    directory = _seed(manager)
    _stop(manager)
    dry = manager.redact("worker", SECRET, dry_run=True)
    assert dry["total_matches"] == 3
    assert dry["left_behind"] == {"keys": 1, "unparseable_lines": 1}
    assert not (manager.registry / "backups").exists()
    result = manager.redact("worker", SECRET)
    assert result["total_matches"] == 3
    assert result["clean"] is False
    document = json.loads((directory / "queue" / "processed" / "m1.json").read_text(encoding="utf-8"))
    assert document == {"text": "the [redacted] and [redacted] again", "count": 42,
                        SECRET: "key name stays"}
    audit_path = manager.registry / "backups" / "redact-audit.jsonl"
    audit = audit_path.read_text(encoding="utf-8")
    assert SECRET not in audit
    assert [json.loads(line)["phase"] for line in audit.splitlines()] == ["started", "completed"]
    assert stat.S_IMODE(audit_path.stat().st_mode) == 0o600
    backups = list((manager.registry / str(result["backup"])).iterdir())
    assert len(backups) == 2
    assert all(stat.S_IMODE(path.stat().st_mode) == 0o600 for path in backups)


def test_redact_refuses_events_while_running_and_short_literals(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    _seed(manager)
    with pytest.raises(AgentDeliveryError, match="not stopped"):
        manager.redact("worker", SECRET)
    with pytest.raises(AgentDeliveryError, match="at least 12"):
        manager.redact("worker", "short", dry_run=True)


def test_redact_refuses_mid_delivery_and_archives_opt_in(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    directory = _seed(manager)
    _stop(manager)
    archived = manager.registry / "archive" / "worker-1795-1234" / "queue" / "processed"
    archived.mkdir(parents=True)
    (archived / "a.json").write_text(json.dumps({"text": f"{SECRET} archived"}), encoding="utf-8")
    assert manager.redact("worker", SECRET)["total_matches"] == 3
    assert SECRET in (archived / "a.json").read_text(encoding="utf-8")
    assert manager.redact("worker", SECRET, include_archived=True)["total_matches"] == 1
    inflight = directory / "queue" / "inflight"
    inflight.mkdir(parents=True, exist_ok=True)
    (inflight / "x.json").write_text("{}", encoding="utf-8")
    with pytest.raises(AgentDeliveryError, match="mid-delivery"):
        manager.redact("worker", SECRET)


def test_redact_supports_a_retired_agent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    _seed(manager)
    _stop(manager)
    archive = manager.registry / "archive" / "worker-1795-1234"
    archive.parent.mkdir(parents=True, exist_ok=True)
    (manager.registry / "agents" / "worker").rename(archive)
    assert manager.redact("worker", SECRET)["total_matches"] == 3


def test_redact_preserves_file_modes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    directory = _seed(manager)
    _stop(manager)
    events = directory / "events" / "e.jsonl"
    processed = directory / "queue" / "processed" / "m1.json"
    events.chmod(0o600)
    processed.chmod(0o640)
    manager.redact("worker", SECRET)
    assert stat.S_IMODE(events.stat().st_mode) == 0o600
    assert stat.S_IMODE(processed.stat().st_mode) == 0o640
