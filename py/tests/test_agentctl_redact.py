"""#1963: agentctl redact in the Python edition."""
from __future__ import annotations

import json
import stat
from pathlib import Path

import pytest

from agentctl.errors import AgentDeliveryError
from .test_herdr_subagents import setup


def _seed(manager: object, root: Path) -> Path:
    directory = Path(manager._directory("worker"))  # type: ignore[attr-defined]
    processed = directory / "queue" / "processed"
    processed.mkdir(parents=True, exist_ok=True)
    (processed / "m1.json").write_text(
        json.dumps({"text": "the SECRET and SECRET again", "count": 42, "SECRET": "key name stays"}),
        encoding="utf-8",
    )
    events = directory / "events"
    events.mkdir(parents=True, exist_ok=True)
    (events / "e.jsonl").write_text('{"note":"a SECRET here"}\nnot json SECRET\n', encoding="utf-8")
    return directory


def test_redact_rewrites_only_string_values_with_backups_and_audit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    directory = _seed(manager, tmp_path)
    dry = manager.redact("worker", "SECRET", dry_run=True)
    assert dry["total_matches"] == 3
    assert not (manager.registry / "backups").exists()
    result = manager.redact("worker", "SECRET")
    assert result["total_matches"] == 3
    rewritten = (directory / "queue" / "processed" / "m1.json").read_text(encoding="utf-8")
    document = json.loads(rewritten)
    assert document == {"text": "the [redacted] and [redacted] again", "count": 42,
                        "SECRET": "key name stays"}
    audit_path = manager.registry / "backups" / "redact-audit.jsonl"
    audit = audit_path.read_text(encoding="utf-8")
    assert "SECRET" not in audit
    assert json.loads(audit)["total_matches"] == 3
    assert stat.S_IMODE(audit_path.stat().st_mode) == 0o600
    backup_dir = manager.registry / str(result["backup"])
    backups = list(backup_dir.iterdir())
    assert len(backups) == 2
    assert all(stat.S_IMODE(path.stat().st_mode) == 0o600 for path in backups)


def test_redact_refuses_mid_delivery_and_archives_opt_in(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    directory = _seed(manager, tmp_path)
    archived = manager.registry / "archive" / "worker-tok" / "queue" / "processed"
    archived.mkdir(parents=True)
    (archived / "a.json").write_text(json.dumps({"text": "SECRET archived"}), encoding="utf-8")
    assert manager.redact("worker", "SECRET")["total_matches"] == 3
    assert "SECRET" in (archived / "a.json").read_text(encoding="utf-8")
    assert manager.redact("worker", "SECRET", include_archived=True)["total_matches"] == 1
    inflight = directory / "queue" / "inflight"
    inflight.mkdir(parents=True, exist_ok=True)
    (inflight / "x.json").write_text("{}", encoding="utf-8")
    with pytest.raises(AgentDeliveryError, match="mid-delivery"):
        manager.redact("worker", "SECRET")
