"""Tests for the tick engine (run_tick) with deterministic fake gate/age probes."""

from __future__ import annotations

from tick_hub.engine import parse_kv_lines, render_emit, run_tick
from tick_hub.model import (
    Emit,
    EmitKind,
    Gate,
    GateWhen,
    HealthCheck,
    Reminder,
    TickConfig,
)
from tick_hub.protocols import GateResult
from tick_hub.state import OpsState


class FakeGate:
    """Return a canned GateResult per command; default success + given stdout."""

    def __init__(self, by_cmd: dict[str, GateResult]) -> None:
        self.by_cmd = by_cmd
        self.calls: list[str] = []

    def run(self, cmd: str) -> GateResult:
        self.calls.append(cmd)
        return self.by_cmd.get(cmd, GateResult(returncode=0, stdout="", ok=True))


class FakeProbe:
    """Return a canned age per glob (None = missing)."""

    def __init__(self, ages: dict[str, int | None]) -> None:
        self.ages = ages

    def newest_age_secs(self, pattern: str, now: int) -> int | None:
        return self.ages.get(pattern)


def _action(name: str, skill: str, *, cadence: int = 0) -> Reminder:
    return Reminder(name, Emit(EmitKind.ACTION, title=f"do {name}", skill=skill), cadence_secs=cadence)


def test_health_status_ok_stale_missing() -> None:
    cfg = TickConfig(
        health_checks=(
            HealthCheck("fresh", "/f", 100, "f"),
            HealthCheck("old", "/o", 100, "o"),
            HealthCheck("gone", "/g", 100, "g"),
        )
    )
    probe = FakeProbe({"/f": 10, "/o": 999, "/g": None})
    result = run_tick(
        cfg, OpsState.default(), now=0, fired={},
        gate_runner=FakeGate({}), age_probe=probe,
    )
    joined = "\n".join(result.lines)
    assert "HEALTH: fresh ok" in joined
    assert "HEALTH: old stale" in joined
    assert "HEALTH: gone missing age_secs=NA" in joined


def test_plain_reminder_fires_and_marks_fired() -> None:
    cfg = TickConfig(reminders=(_action("sync", "git-sync", cadence=3600),))
    result = run_tick(
        cfg, OpsState.default(), now=500, fired={},
        gate_runner=FakeGate({}), age_probe=FakeProbe({}),
    )
    assert any(ln.startswith("ACTION: git-sync") for ln in result.lines)
    assert result.fired["sync"] == 500  # cadence clock reset
    assert result.actions_emitted == 1


def test_not_due_reminder_is_silent_and_unchanged() -> None:
    cfg = TickConfig(reminders=(_action("sync", "git-sync", cadence=3600),))
    result = run_tick(
        cfg, OpsState.default(), now=1000, fired={"sync": 900},
        gate_runner=FakeGate({}), age_probe=FakeProbe({}),
    )
    assert not any("git-sync" in ln for ln in result.lines)
    assert result.fired == {"sync": 900}  # untouched


def test_gate_success_fires_failure_suppresses() -> None:
    rem = Reminder(
        "ci",
        Emit(EmitKind.ACTION, title="ci red", skill="ci-red"),
        gate=Gate(cmd="check", when=GateWhen.FAILURE),
    )
    cfg = TickConfig(reminders=(rem,))
    # gate exits 0 -> when=failure NOT satisfied -> no emit, but the check ran (fired stamped).
    ok = run_tick(
        cfg, OpsState.default(), now=5, fired={},
        gate_runner=FakeGate({"check": GateResult(0, "", True)}), age_probe=FakeProbe({}),
    )
    assert not any("ci-red" in ln for ln in ok.lines)
    assert ok.fired["ci"] == 5
    # gate exits 1 -> when=failure satisfied -> emit.
    red = run_tick(
        cfg, OpsState.default(), now=5, fired={},
        gate_runner=FakeGate({"check": GateResult(1, "", True)}), age_probe=FakeProbe({}),
    )
    assert any("ci-red" in ln for ln in red.lines)


def test_gate_capture_interpolates_fields_and_title() -> None:
    rem = Reminder(
        "backlog",
        Emit(
            EmitKind.ACTION,
            title="{count} ready (>{threshold})",
            skill="triage",
            fields={"threshold": "20"},
        ),
        gate=Gate(cmd="count", when=GateWhen.ALWAYS, capture=True),
    )
    cfg = TickConfig(reminders=(rem,))
    result = run_tick(
        cfg, OpsState.default(), now=0, fired={},
        gate_runner=FakeGate({"count": GateResult(0, "count=42\n", True)}),
        age_probe=FakeProbe({}),
    )
    line = next(ln for ln in result.lines if ln.startswith("ACTION: triage"))
    assert "count=42" in line
    assert 'title="42 ready (>20)"' in line


def test_gate_run_failure_emits_error_no_fired_stamp() -> None:
    rem = Reminder("x", Emit(EmitKind.ACTION, title="x", skill="x"), gate=Gate(cmd="boom"))
    cfg = TickConfig(reminders=(rem,))
    result = run_tick(
        cfg, OpsState.default(), now=9, fired={},
        gate_runner=FakeGate({"boom": GateResult(-1, "", False, error="not found")}),
        age_probe=FakeProbe({}),
    )
    assert any(ln.startswith("ERROR: reminder x") for ln in result.lines)
    assert "x" not in result.fired  # not marked -> retried next tick


def test_requires_flags_gating() -> None:
    rem = Reminder(
        "bench",
        Emit(EmitKind.ACTION, title="run bench", skill="run-benchmark"),
        requires_flags=("benchmark_enabled",),
    )
    cfg = TickConfig(reminders=(rem,))
    off = OpsState(enabled=True, tick_frequency_min=30, flags={"benchmark_enabled": False})
    r_off = run_tick(
        cfg, off, now=0, fired={}, gate_runner=FakeGate({}), age_probe=FakeProbe({})
    )
    assert not any("run-benchmark" in ln for ln in r_off.lines)
    assert "bench" not in r_off.fired  # suppressed reminders do not consume the cadence
    on = OpsState(enabled=True, tick_frequency_min=30, flags={"benchmark_enabled": True})
    r_on = run_tick(
        cfg, on, now=0, fired={}, gate_runner=FakeGate({}), age_probe=FakeProbe({})
    )
    assert any("run-benchmark" in ln for ln in r_on.lines)


def test_disabled_state_runs_health_but_no_reminders() -> None:
    cfg = TickConfig(
        reminders=(_action("sync", "git-sync"),),
        health_checks=(HealthCheck("db", "/db", 100, "d"),),
    )
    state = OpsState(enabled=False, tick_frequency_min=30)
    result = run_tick(
        cfg, state, now=0, fired={},
        gate_runner=FakeGate({}), age_probe=FakeProbe({"/db": 5}),
    )
    assert any(ln.startswith("HEALTH: db ok") for ln in result.lines)
    assert not any("git-sync" in ln for ln in result.lines)
    assert any("disabled" in ln for ln in result.lines)
    assert result.actions_emitted == 0


def test_trailing_summary_counts_actions() -> None:
    cfg = TickConfig(reminders=(_action("a", "sa"), _action("b", "sb")))
    result = run_tick(
        cfg, OpsState.default(), now=0, fired={},
        gate_runner=FakeGate({}), age_probe=FakeProbe({}),
    )
    assert result.actions_emitted == 2
    assert result.lines[-1] == "NOTE: emitted 2 instruction(s) this tick"


def test_note_reminder_is_note_not_action() -> None:
    cfg = TickConfig(
        reminders=(Reminder("n", Emit(EmitKind.NOTE, title="just a note")),)
    )
    result = run_tick(
        cfg, OpsState.default(), now=0, fired={},
        gate_runner=FakeGate({}), age_probe=FakeProbe({}),
    )
    assert "NOTE: just a note" in result.lines
    assert result.actions_emitted == 0


def test_parse_kv_lines() -> None:
    assert parse_kv_lines("a=1\n# c\n\nb = two words \nno-eq\n") == {"a": "1", "b": "two words"}


def test_render_emit_note_interpolates() -> None:
    line = render_emit(Emit(EmitKind.NOTE, title="hi {who}"), {"who": "there"})
    assert line == "NOTE: hi there"
