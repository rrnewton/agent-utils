"""Version-skew tolerance: a newer writer must not brick older readers (#1788).

The reported outage: an older pinned reader running the read-only
``classify-create-journals`` refused the whole registry because a newer
writer had added handoff accounting keys, and the append-only log made
that refusal permanent. The rule implemented here: load and replay keep
every record with unknown keys preserved raw and warn once; only a
command that acts on the affected slot (remove/finish of that slot)
refuses, with a typed newer-writer refusal naming the slot.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    command,
    control_directory,
    create,
    finish,
    make_project,
    raw_command,
)

FUTURE_KEY = "future_accounting"
FUTURE_VALUE = {"entries": [{"path": "notes.txt", "bytes": 3}]}


def _project_with_newer_handoff(tmp_path: Path) -> Path:
    project, _repository, _remote = make_project(tmp_path)
    created = create(project)
    assert created.returncode == 0, created.stderr
    finished = finish(project)
    assert finished.returncode == 0, finished.stderr
    control = control_directory(project)
    active_path = control / "ACTIVE.testhost.json"
    active = json.loads(active_path.read_text(encoding="utf-8"))
    assert active["slots"][0]["handoff"] is not None
    active["slots"][0]["handoff"][FUTURE_KEY] = FUTURE_VALUE
    active_path.write_text(json.dumps(active), encoding="utf-8")
    events_dir = control / "EVENTS.testhost"
    for event_path in sorted(events_dir.glob("*.json")):
        event = json.loads(event_path.read_text(encoding="utf-8"))
        record = event.get("payload", {}).get("record")
        if isinstance(record, dict) and isinstance(record.get("handoff"), dict):
            record["handoff"][FUTURE_KEY] = FUTURE_VALUE
            core = {k: v for k, v in event.items() if k != "sha256"}
            event["sha256"] = cli._event_digest(core)
            event_path.write_text(json.dumps(event), encoding="utf-8")
    return project


def test_old_reader_classify_tolerates_newer_handoff_keys(tmp_path: Path) -> None:
    """Today's case: classify over newer handoff keys succeeds."""
    project = _project_with_newer_handoff(tmp_path)
    admitted = raw_command(
        project,
        "classify-create-journals",
        "slot02",
        "--slot-type",
        "validate",
        "--agent",
        "validate-2",
        "--format",
        "json",
    )
    assert admitted.returncode == 0, admitted.stderr
    assert "invalid fields" not in admitted.stderr


def test_remove_refuses_newer_handoff_for_that_slot_only(tmp_path: Path) -> None:
    project = _project_with_newer_handoff(tmp_path)
    refused = command(project, "remove", "slot01", "--coordinator-pid", str(__import__("os").getpid()), "--expected-generation", "1")
    assert refused.returncode == 3, refused.stderr
    assert "NEWER WRITER" in refused.stderr
    assert "slot01" in refused.stderr
    assert FUTURE_KEY in refused.stderr
    assert "invalid fields" not in refused.stderr


def test_newer_handoff_keys_survive_a_load_write_round_trip(tmp_path: Path) -> None:
    project = _project_with_newer_handoff(tmp_path)
    config = cli._load_config(str(project), "testhost")
    state = cli._load_active_snapshot(config)
    record = state.slots[0]
    assert record.handoff is not None
    written = cli._record_to_obj(record)
    handoff_obj = written["handoff"]
    assert isinstance(handoff_obj, dict)
    assert handoff_obj[FUTURE_KEY] == FUTURE_VALUE


def test_ignored_summary_in_active_record_is_tolerated_then_refused_at_remove(
    tmp_path: Path,
) -> None:
    """The summary field in the active record: permanent history (#1772)."""
    project = _project_with_newer_handoff(tmp_path)
    control = control_directory(project)
    active_path = control / "ACTIVE.testhost.json"
    active = json.loads(active_path.read_text(encoding="utf-8"))
    active["slots"][0]["handoff"]["ignored_summary"] = {"count": 1, "bytes": 3}
    active_path.write_text(json.dumps(active), encoding="utf-8")
    admitted = raw_command(
        project,
        "classify-create-journals",
        "slot02",
        "--slot-type",
        "validate",
        "--agent",
        "validate-2",
        "--format",
        "json",
    )
    assert admitted.returncode == 0, admitted.stderr
    config = cli._load_config(str(project), "testhost")
    record = cli._load_active_snapshot(config).slots[0]
    handoff_obj = cli._record_to_obj(record)["handoff"]
    assert isinstance(handoff_obj, dict)
    assert handoff_obj["ignored_summary"] == {"count": 1, "bytes": 3}
    with pytest.raises(cli.SchemaSkewRefusal):
        cli._assert_record_actable(record, "remove")


def test_finish_journal_sidecar_key_is_tolerated_in_record_copy() -> None:
    """The sidecar key in a finish journal's record copy: transient (#1757)."""
    checkout = {
        "name": "product",
        "path": "worktrees/slot01/product",
        "repository": "repo",
        "branch": "b",
        "start_point": "a" * 40,
        "remote": "origin",
        "remote_url_sha256": "b" * 64,
        "landed_ref": "refs/remotes/origin/main",
        "head": "c" * 40,
        "containing_remote_refs": [],
        "vcs": "git",
        "containing_remote_refs_sidecar": {
            "sha256": "d" * 64,
            "count": 100,
            "path": "containing-refs/x.json",
        },
    }
    parsed = cli._checkout_from_obj(checkout, "finish journal.record.checkouts[0]")
    assert cli._checkout_to_obj(parsed)["containing_remote_refs_sidecar"] == checkout[
        "containing_remote_refs_sidecar"
    ]


def test_writer_ceiling_refuses_less_safe_fallback(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config_path = project / ".wrkslots.yml"
    text = config_path.read_text(encoding="utf-8")
    config_path.write_text(text + "\nwriter_ceiling: 1\nreader_floor: 1\n", encoding="utf-8")
    config = cli._load_config(str(project), "testhost")
    assert cli._writer_ceiling(config) == 1
    with pytest.raises(cli.Refusal) as caught:
        cli._assert_writer_level(config, 2, "handoff ignored-file accounting")
    assert "writer_ceiling" in str(caught.value)
