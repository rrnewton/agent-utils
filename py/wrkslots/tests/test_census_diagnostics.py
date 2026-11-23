"""#2082: census stderr budget derivation and timeout remedy text.

The validate-remove stderr budget scales with the live process count up
to a cap (derivation in `_validate_remove_batch_census_stderr_bytes`),
and census refusals name the slot, the phase, the failure shape and
the next step.
"""

from __future__ import annotations

import pytest

from wrkslots import cli


def test_stderr_budget_uses_live_host_count_by_default(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    floor = cli._VALIDATE_REMOVE_BATCH_CENSUS_STDERR_BYTES
    cap = cli._VALIDATE_REMOVE_BATCH_CENSUS_STDERR_CAP_BYTES
    per_process = cli._ABSENT_CENSUS_STDERR_PER_PROCESS_BYTES
    for count, expected in (
        (0, floor),
        (256, max(floor, 256 * per_process)),
        (6500, min(cap, 6500 * per_process)),
        (10**9, cap),
    ):
        monkeypatch.setattr(cli, "_host_process_count", lambda count=count: count)
        assert cli._validate_remove_batch_census_stderr_bytes(1) == expected


def test_stderr_budget_floor_scale_and_cap() -> None:
    floor = cli._VALIDATE_REMOVE_BATCH_CENSUS_STDERR_BYTES
    cap = cli._VALIDATE_REMOVE_BATCH_CENSUS_STDERR_CAP_BYTES
    # Floor on a quiet host, one allowance per sealed slot.
    assert cli._validate_remove_batch_census_stderr_bytes(1, process_count=10) == floor
    assert cli._validate_remove_batch_census_stderr_bytes(3, process_count=10) == 3 * floor
    # Scales with the process count (256 bytes per process, as the
    # absent-slot census allows) once past the floor.
    scaled = cli._validate_remove_batch_census_stderr_bytes(1, process_count=2000)
    assert scaled == 2000 * cli._ABSENT_CENSUS_STDERR_PER_PROCESS_BYTES
    # Capped per slot however busy the host is.
    assert cli._validate_remove_batch_census_stderr_bytes(1, process_count=10**9) == cap
    assert cli._validate_remove_batch_census_stderr_bytes(2, process_count=10**9) == 2 * cap
    assert cli._validate_remove_batch_census_stderr_bytes(0, process_count=10) == floor


def _context_budget() -> cli._ReadOnlyCommandBudget:
    return cli._ReadOnlyCommandBudget.start(
        timeout_seconds=0.05,
        stdout_limit=1024,
        stderr_limit=1024,
        census_context="slot alpha (validate-remove census)",
        rerun_census_seconds=120,
    )


def test_timeout_refusal_names_slot_phase_shape_and_next_step() -> None:
    budget = _context_budget()
    budget.deadline -= 10  # already expired, deterministically
    with pytest.raises(cli.Refusal) as caught:
        budget.remaining_seconds()
    text = str(caught.value)
    assert "operation-wide time bound" in text  # historical wording kept
    assert "slot alpha" in text
    assert "validate-remove census" in text
    assert "failure: time bound exceeded" in text
    assert "next step:" in text
    assert "--census-seconds 120" in text


def test_output_bound_refusal_names_budget_shape() -> None:
    budget = _context_budget()
    with pytest.raises(cli.Refusal) as caught:
        budget.consume_output(b"", b"x" * 2048)
    text = str(caught.value)
    assert "stderr over by" in text
    assert "failure: budget exceeded" in text
    assert "slot alpha" in text


def test_bare_budget_keeps_bare_text() -> None:
    budget = cli._ReadOnlyCommandBudget.start(
        timeout_seconds=60, stdout_limit=8, stderr_limit=8
    )
    with pytest.raises(cli.Refusal) as caught:
        budget.consume_output(b"", b"x" * 9)
    assert "failure:" not in str(caught.value)
