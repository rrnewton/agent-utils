"""The scoped-status event index (#2039).

Scoped status must not parse the whole event log: the writer keeps a
per-machine index of the latest event sequence per slot and kind, and
the reader trusts it only when its tip is exactly the chain tip and
the ACTIVE snapshot revision matches.  Anything else falls back to
today's full replay.  No elapsed-time budget decides any of this;
bounded reads are proven by making the full parse fail loudly.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import create, make_project


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _index(config: wrkslots.Config) -> dict[str, object]:
    directory = wrkslots._event_directory(config, "testhost")
    index = wrkslots._load_event_index(directory, "testhost")
    assert index is not None
    return index


def _tip(index: dict[str, object]) -> dict[str, object]:
    tip = index["tip"]
    assert isinstance(tip, dict)
    return tip


def _entry(index: dict[str, object], slot: str) -> dict[str, object]:
    slots = index["slots"]
    assert isinstance(slots, dict)
    entry = slots[slot]
    assert isinstance(entry, dict)
    return entry


def _active(payload: dict[str, object]) -> list[dict[str, object]]:
    active = payload["active"]
    assert isinstance(active, list)
    return active


def _status_json(
    project: Path, *args: str, capsys: pytest.CaptureFixture[str]
) -> tuple[int, dict[str, object]]:
    result = wrkslots.main(
        ["--project-root", str(project), "status", "--format", "json", *args]
    )
    captured = capsys.readouterr()
    return result, json.loads(captured.out)


def test_index_follows_appends(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    index = _index(config)
    assert isinstance(_tip(index)["sequence"], int)
    writer = wrkslots._event_writer(config, "testhost")
    writer.append("slot-held", {"slot": "slot01", "reason": "probe"})
    index = _index(config)
    assert _entry(index, "slot01")["held"] == _tip(index)["sequence"]
    writer = wrkslots._event_writer(config, "testhost")
    writer.append("slot-hold-released", {"slot": "slot01"})
    index = _index(config)
    assert _entry(index, "slot01")["hold_released"] == _tip(index)["sequence"]


def test_scoped_status_matches_full_without_parsing_log(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    writer = wrkslots._event_writer(config, "testhost")
    writer.append("slot-held", {"slot": "slot01", "reason": "probe"})

    def _no_parse(*_args: object, **_kwargs: object) -> tuple[object, ...]:
        raise AssertionError("scoped status parsed the event log")

    monkeypatch.setattr(wrkslots, "_load_events", _no_parse)
    scoped_rc, scoped = _status_json(project, "--slot", "slot01", capsys=capsys)
    monkeypatch.undo()
    full_rc, full = _status_json(project, "--slot", "slot01", capsys=capsys)
    assert scoped_rc == 0 and full_rc == 0
    assert _active(scoped) == _active(full)
    assert _active(scoped)[0]["held"] is True


def test_old_build_append_then_new_append_rebuilds_index(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    """An interleaved old-build append must not leave a stale slot row.

    The old build writes an event file without touching the index; the
    next new-build append sees the continuity break and rebuilds, so
    the interleaved event's slot entry is present and correct.
    """

    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    directory = wrkslots._event_directory(config, "testhost")
    names = wrkslots._event_file_names(directory)
    previous = json.loads((directory / names[-1]).read_text())["sha256"]
    core = {
        "schema": wrkslots.EVENT_SCHEMA,
        "machine": "testhost",
        "sequence": len(names) + 1,
        "previous_sha256": previous,
        "recorded_at": "2026-11-21T00:00:00-05:00",
        "kind": "slot-held",
        "payload": {"slot": "slot01", "reason": "old-build"},
    }
    event = {**core, "sha256": wrkslots._event_digest(core)}
    wrkslots._atomic_write_json(
        directory / f"{len(names) + 1:020d}.json", event
    )
    # The index is now stale: its tip no longer matches the chain.
    stale = _index(config)
    assert _tip(stale)["sequence"] == len(names)

    writer = wrkslots._event_writer(config, "testhost")
    writer.append("slot-hold-released", {"slot": "slot01"})
    index = _index(config)
    assert _tip(index)["sequence"] == len(names) + 2
    assert _entry(index, "slot01")["held"] == len(names) + 1
    assert _entry(index, "slot01")["hold_released"] == len(names) + 2

    rc, payload = _status_json(project, "--slot", "slot01", capsys=capsys)
    assert rc == 0
    assert _active(payload)[0]["held"] is False


def test_scoped_status_falls_back_when_index_tip_is_wrong(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    index_path = wrkslots._event_index_path(
        wrkslots._event_directory(config, "testhost"), "testhost"
    )
    index = json.loads(index_path.read_text())
    index["tip"] = {"sequence": 1, "sha256": "0" * 64}
    index_path.write_text(json.dumps(index))
    rc, payload = _status_json(project, "--slot", "slot01", capsys=capsys)
    assert rc == 0
    assert _active(payload)[0]["slot"] == "slot01"


def test_scoped_status_reads_bounded_events_on_a_long_log(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    for index in range(300):
        writer = wrkslots._event_writer(config, "testhost")
        writer.append(
            "slot-held" if index % 2 == 0 else "slot-hold-released",
            {"slot": "slot02", "reason": "filler"},
        )

    def _no_parse(*_args: object, **_kwargs: object) -> tuple[object, ...]:
        raise AssertionError("scoped status parsed the event log")

    monkeypatch.setattr(wrkslots, "_load_events", _no_parse)
    rc, payload = _status_json(project, "--slot", "slot01", capsys=capsys)
    assert rc == 0
    assert _active(payload)[0]["slot"] == "slot01"
