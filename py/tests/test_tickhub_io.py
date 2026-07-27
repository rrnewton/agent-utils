"""Tests for tick_hub.io (config JSON round-trip + strict parse errors)."""

from __future__ import annotations

from tick_hub.io import (
    TickConfigError,
    config_from_json,
    config_to_json,
)
from tick_hub.model import (
    Emit,
    EmitKind,
    Gate,
    GateWhen,
    HealthCheck,
    Reminder,
    TickConfig,
)


def _cfg() -> TickConfig:
    return TickConfig(
        description="the whole tick",
        reminders=(
            Reminder(
                name="git_sync",
                emit=Emit(EmitKind.ACTION, title="sync now", skill="git-sync"),
                cadence_secs=21600,
            ),
            Reminder(
                name="backlog",
                emit=Emit(
                    EmitKind.ACTION,
                    title="{count} items",
                    skill="triage",
                    fields={"threshold": "20"},
                ),
                requires_flags=("ops_in_charge",),
                gate=Gate(cmd="echo count=3", when=GateWhen.ALWAYS, capture=True),
            ),
        ),
        health_checks=(
            HealthCheck(name="db", glob="/var/*.sql", threshold_secs=3600, detail="snap"),
        ),
    )


def test_roundtrip_is_stable() -> None:
    cfg = _cfg()
    once = config_to_json(cfg)
    twice = config_to_json(config_from_json(once))
    assert once == twice  # canonical JSON is a fixed point
    back = config_from_json(once)
    assert [r.name for r in back.reminders] == ["git_sync", "backlog"]
    assert back.description == "the whole tick"
    assert back.reminders[0].cadence_secs == 21600
    assert back.reminders[1].requires_flags == ("ops_in_charge",)
    gate = back.reminders[1].gate
    assert gate is not None and gate.when is GateWhen.ALWAYS and gate.capture is True
    assert back.reminders[1].emit.fields == {"threshold": "20"}
    assert back.health_checks[0].glob == "/var/*.sql"


def test_minimal_document_defaults() -> None:
    cfg = config_from_json('{"reminders": [{"name": "r", "emit": {"skill": "s"}}]}')
    rem = cfg.reminders[0]
    assert rem.name == "r"
    assert rem.cadence_secs == 0 and rem.requires_flags == () and rem.gate is None
    assert rem.emit.kind is EmitKind.ACTION and rem.emit.skill == "s"
    assert cfg.health_checks == () and cfg.description == ""


def test_note_reminder_needs_no_skill() -> None:
    cfg = config_from_json(
        '{"reminders": [{"name": "n", "emit": {"kind": "note", "title": "hi"}}]}'
    )
    assert cfg.reminders[0].emit.kind is EmitKind.NOTE
    assert cfg.reminders[0].emit.title == "hi"


def test_empty_config_is_valid() -> None:
    cfg = config_from_json("{}")
    assert cfg.reminders == () and cfg.health_checks == ()


def test_strict_parse_errors() -> None:
    bad_docs = [
        "not json at all",
        "[]",  # root not an object
        '{"reminders": "not a list"}',
        '{"health_checks": "not a list"}',
        '{"reminders": [{"emit": {"skill": "s"}}]}',  # missing name
        '{"reminders": [{"name": "r"}]}',  # missing emit
        '{"reminders": [{"name": "r", "emit": {"kind": "note", "title": 5}}]}',  # title not str
        '{"reminders": [{"name": "r", "emit": {"kind": "nope", "title": "t"}}]}',  # bad kind
        '{"reminders": [{"name": "r", "emit": {"kind": "action"}}]}',  # ACTION needs skill
        '{"reminders": [{"name": "r", "cadence_secs": "x", "emit": {"skill": "s"}}]}',
        '{"reminders": [{"name": "r", "requires_flags": [1], "emit": {"skill": "s"}}]}',
        '{"reminders": [{"name": "r", "gate": {"when": "nope"}, "emit": {"skill": "s"}}]}',
        '{"reminders": [{"name": "r", "gate": {"cmd": "c", "when": "x"}, "emit": {"skill": "s"}}]}',
        '{"health_checks": [{"name": "h"}]}',  # missing glob
        '{"reminders": [{"name": "r", "emit": {"skill": "s", "fields": {"k": 1}}}]}',
    ]
    for doc in bad_docs:
        raised = False
        try:
            config_from_json(doc)
        except TickConfigError:
            raised = True
        assert raised, f"expected TickConfigError for: {doc!r}"
