"""The tick engine: turn (config + state + fired-state + now) into the emitted lines.

One call to :func:`run_tick` is one heartbeat. It:

1. evaluates every :class:`~tick_hub.model.HealthCheck` (a HEALTH line each),
2. emits the ops-state machine's own lines (summary NOTE, tick-frequency actualization, disabled
   NOTE) via :func:`~tick_hub.state.state_lines`,
3. when the hub is enabled, checks each DUE reminder whose ``requires_flags`` are all truthy: runs
   its optional gate, records that the check ran, and emits its ACTION/NOTE when the gate fires, and
4. appends a trailing NOTE with the count of instructions emitted.

The heavy lifting (running gate commands, measuring file ages) is delegated to the pluggable
:mod:`tick_hub.protocols` boundaries, so the ordering / gating / emission logic here is pure and
deterministic given ``now`` and the injected probes.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass

from tick_hub.cadence import is_due
from tick_hub.emit import (
    HEALTH_STATUS_MISSING,
    HEALTH_STATUS_OK,
    HEALTH_STATUS_STALE,
    format_action,
    format_error,
    format_health,
    format_note,
)
from tick_hub.model import Emit, EmitKind, Gate, GateWhen, HealthCheck, Reminder, TickConfig
from tick_hub.protocols import FileAgeProbe, GateRunner
from tick_hub.state import OpsState, flag_truthy, state_lines


@dataclass(frozen=True)
class TickResult:
    """Everything one tick produced: the emitted lines, the new fired-state, the ACTION count."""

    lines: tuple[str, ...]
    fired: Mapping[str, int]
    actions_emitted: int


def parse_kv_lines(text: str) -> dict[str, str]:
    """Parse a gate command's stdout ``key=value`` lines (blank / ``#`` lines ignored)."""
    out: dict[str, str] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" in line:
            key, _, value = line.partition("=")
            key = key.strip()
            if key:
                out[key] = value.strip()
    return out


def evaluate_health(hc: HealthCheck, probe: FileAgeProbe, now: int) -> str:
    age = probe.newest_age_secs(hc.glob, now)
    if age is None:
        status = HEALTH_STATUS_MISSING
    elif age <= hc.threshold_secs:
        status = HEALTH_STATUS_OK
    else:
        status = HEALTH_STATUS_STALE
    return format_health(hc.name, status, age, hc.threshold_secs, hc.detail)


def _gate_fires(when: GateWhen, returncode: int, stdout: str) -> bool:
    if when is GateWhen.SUCCESS:
        return returncode == 0
    if when is GateWhen.FAILURE:
        return returncode != 0
    if when is GateWhen.NONEMPTY:
        return stdout.strip() != ""
    return True  # GateWhen.ALWAYS


def _eval_gate(
    gate: Gate | None, runner: GateRunner
) -> tuple[bool, dict[str, str], str | None]:
    """Return (fire, captured_fields, error). ``error`` non-None means the gate could not run."""
    if gate is None:
        return True, {}, None
    result = runner.run(gate.cmd)
    if not result.ok:
        return False, {}, f"gate command could not run ({gate.cmd!r}): {result.error}"
    captured = parse_kv_lines(result.stdout) if gate.capture else {}
    return _gate_fires(gate.when, result.returncode, result.stdout), captured, None


def _interpolate(text: str, captured: Mapping[str, str]) -> str:
    for key, value in captured.items():
        text = text.replace("{" + key + "}", value)
    return text


def render_emit(emit: Emit, captured: Mapping[str, str]) -> str:
    """Render a fired reminder's :class:`~tick_hub.model.Emit` into a line.

    Captured gate values override matching static fields; then ``{key}`` placeholders in the title
    and in each field value are resolved from the merged set (static fields + captured), so a title
    can reference both a static ``{threshold}`` and a captured ``{count}``."""
    merged: dict[str, str] = {}
    for key, value in emit.fields.items():
        merged[key] = _interpolate(value, captured)
    for key, value in captured.items():
        merged[key] = value
    title = _interpolate(emit.title, merged)
    if emit.kind is EmitKind.NOTE:
        return format_note(title)
    return format_action(emit.skill, merged, title)


def _flags_satisfied(required: Sequence[str], flags: Mapping[str, object]) -> bool:
    typed_flags = {k: v for k, v in flags.items() if isinstance(v, (bool, int, str))}
    return all(flag_truthy(typed_flags, name) for name in required)


def run_tick(
    config: TickConfig,
    state: OpsState,
    *,
    now: int,
    fired: Mapping[str, int],
    gate_runner: GateRunner,
    age_probe: FileAgeProbe,
    current_tick_min: int | None = None,
) -> TickResult:
    """Run one tick and return the emitted lines plus the advanced fired-state (pure w.r.t. I/O,
    which is confined to the injected ``gate_runner`` / ``age_probe``)."""
    lines: list[str] = []
    actions = 0

    for hc in config.health_checks:
        lines.append(evaluate_health(hc, age_probe, now))

    for line in state_lines(state, current_tick_min):
        lines.append(line)
        if line.startswith("ACTION: "):
            actions += 1

    new_fired = dict(fired)
    if state.enabled:
        for rem in config.reminders:
            if not is_due(rem.name, rem.cadence_secs, now, fired):
                continue
            if not _flags_satisfied(rem.requires_flags, state.flags):
                # Flag-suppressed: do NOT consume the cadence, so it fires promptly once enabled.
                continue
            fire, captured, error = _eval_gate(rem.gate, gate_runner)
            if error is not None:
                # The check did not complete: surface it and retry next tick (no fired stamp).
                lines.append(format_error(f"reminder {rem.name}: {error}"))
                continue
            new_fired[rem.name] = now  # the check ran; the cadence clock resets
            if not fire:
                continue
            line = render_emit(rem.emit, captured)
            lines.append(line)
            if line.startswith("ACTION: "):
                actions += 1

    lines.append(format_note(f"emitted {actions} instruction(s) this tick"))
    return TickResult(tuple(lines), new_fired, actions)


__all__ = [
    "TickResult",
    "run_tick",
    "evaluate_health",
    "render_emit",
    "parse_kv_lines",
    "HEALTH_STATUS_OK",
    "HEALTH_STATUS_STALE",
    "HEALTH_STATUS_MISSING",
]
