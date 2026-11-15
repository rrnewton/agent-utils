"""Checked lifecycle-model tests (#1568).

The model in ``wrkslots.lifecyclemodel`` is explored exhaustively.
Baseline runs must reproduce every observed leak class as a reachable
state and as a scheduled-liveness counterexample; all-fix runs must
close liveness for the finisher's domain (dead owner, unoccupied, no
unread handoff).  Named manual terminals stay outside that domain by
construction: an unread handoff needs a human reader, a leftover
process needs its own supervisor, and a directory without a row has
no row to archive.
"""

from __future__ import annotations

import pytest

from wrkslots import lifecyclemodel as lm

BASELINE = lm.Scenario()
ALLFIX = lm.Scenario(fixes=lm.ALL_FIXES)


@pytest.fixture(scope="module")
def baseline() -> lm.Exploration:
    return lm.explore(BASELINE)


@pytest.fixture(scope="module")
def allfix() -> lm.Exploration:
    return lm.explore(ALLFIX)


@pytest.fixture(scope="module")
def validate_baseline() -> lm.Exploration:
    return lm.explore(lm.Scenario(slot_type="validate", mirror_fails=1))


@pytest.fixture(scope="module")
def validate_allfix() -> lm.Exploration:
    return lm.explore(
        lm.Scenario(slot_type="validate", mirror_fails=1, fixes=lm.ALL_FIXES)
    )


def _fail_classes(ex: lm.Exploration) -> set[str]:
    return {lm.why_stayed(s, ex.scenario) for s in ex.liveness_failures()}


def test_every_observed_class_is_reachable(
    baseline: lm.Exploration, validate_baseline: lm.Exploration
) -> None:
    seen = set(baseline.classes()) | set(validate_baseline.classes())
    expected = {
        "HELD",
        "JOURNAL_BLOCKED_DEAD_OWNER",
        "INTERRUPTED_CREATE_JOURNAL",
        "ROW_WITHOUT_DIRECTORY",
        "DIRECTORY_WITHOUT_ROW",
        "UNREAD_HANDOFF",
        "LEFTOVER_PROCESS_IN_DELETED_CWD",
        "LIVE_OWNER_OR_OCCUPANT",
        "BRANCH_DRIFT_OR_DETACHED",
        "PRIVATE_OR_UNPUSHED_HISTORY",
        "NO_OWNER_GENERATION_INDETERMINATE",
        "NESTED_GIT_REGISTRATION_WITHOUT_ROW",
        "VALIDATE_LEDGER_NOT_PUBLISHED_PROVEN",
        "RETIREMENT_QUEUE_PENDING_NO_REMOVER",
        "DEAD_DELETABLE_NO_AUTOMATIC_REMOVER",
    }
    assert expected <= seen
    # A slot appearing during an audit scan is a measurement race with
    # verdict UNKNOWN, not a lifecycle state; it is ruled out here and
    # covered by the fail-closed audit tests instead.


def test_baseline_liveness_fails_for_each_leak_class(
    baseline: lm.Exploration, validate_baseline: lm.Exploration
) -> None:
    fails = _fail_classes(baseline)
    assert {
        "HELD",
        "DEAD_DELETABLE_NO_AUTOMATIC_REMOVER",
        "RETIREMENT_QUEUE_PENDING_NO_REMOVER",
        "PRIVATE_OR_UNPUSHED_HISTORY",
        "ROW_WITHOUT_DIRECTORY",
        "BRANCH_DRIFT_OR_DETACHED",
        "NESTED_GIT_REGISTRATION_WITHOUT_ROW",
        "NO_OWNER_GENERATION_INDETERMINATE",
        "JOURNAL_BLOCKED_DEAD_OWNER",
    } <= fails
    assert "VALIDATE_LEDGER_NOT_PUBLISHED_PROVEN" in _fail_classes(
        validate_baseline
    )


def test_all_fixes_close_scheduled_liveness(
    allfix: lm.Exploration, validate_allfix: lm.Exploration
) -> None:
    assert allfix.liveness_failures() == []
    assert validate_allfix.liveness_failures() == []
    other = lm.explore(lm.Scenario(other_journal=1, fixes=lm.ALL_FIXES))
    assert other.liveness_failures() == []
    blocked = lm.explore(lm.Scenario(other_journal=1))
    assert blocked.liveness_failures() != []


def test_counterexample_trace_names_actions(
    baseline: lm.Exploration,
) -> None:
    failures = baseline.liveness_failures()
    assert failures
    trace = baseline.trace(failures[0])
    assert trace and all(isinstance(step, str) for step in trace)


def _viol_entry_steps(ex: lm.Exploration) -> list[tuple[str, str]]:
    steps = []
    for state in ex.safety_violations():
        trace = ex.trace(state)
        steps.append((trace[-1] if trace else "init", state.viol))
    return steps


def test_no_path_violates_safety(
    baseline: lm.Exploration, allfix: lm.Exploration
) -> None:
    # Remove, recover-absent, and TTL teardown (handoff and
    # occupancy gates included) never archive a live, held,
    # unread, or occupied slot's present storage.
    assert baseline.safety_violations() == []
    assert allfix.safety_violations() == []


def test_ttl_unfixed_reproduces_the_historical_bug() -> None:
    ex = lm.explore(lm.Scenario(bugs=frozenset({lm.BUG_TTL_UNFIXED})))
    tags = {viol for _step, viol in _viol_entry_steps(ex)}
    assert {"live-owner", "held"} <= tags


def test_audit_hidden_history_disagreement() -> None:
    def hidden_deletable(ex: lm.Exploration) -> int:
        return sum(
            1
            for s in ex.states
            if s.history == "hidden"
            and lm.audit_verdict(s, ex.scenario) == "DELETABLE"
        )

    assert hidden_deletable(lm.explore(BASELINE)) > 0
    fixed = lm.explore(lm.Scenario(fixes=frozenset({lm.AUDIT_SUBMODULE})))
    assert hidden_deletable(fixed) == 0


def test_each_major_fix_is_necessary() -> None:
    without_finisher = lm.explore(
        lm.Scenario(fixes=lm.ALL_FIXES - {lm.FINISHER})
    )
    assert without_finisher.liveness_failures() != []

    without_expiry = lm.explore(
        lm.Scenario(fixes=lm.ALL_FIXES - {lm.HOLD_EXPIRY})
    )
    assert "HELD" in _fail_classes(without_expiry)

    without_delegate = lm.explore(
        lm.Scenario(fixes=lm.ALL_FIXES - {lm.DELEGATED})
    )
    assert without_delegate.liveness_failures() != []
    assert any(
        s.coord == "restarted" for s in without_delegate.liveness_failures()
    )

    without_abandon = lm.explore(
        lm.Scenario(
            slot_type="validate",
            mirror_fails=1,
            fixes=lm.ALL_FIXES - {lm.LEDGER_ABANDON},
        )
    )
    assert "VALIDATE_LEDGER_NOT_PUBLISHED_PROVEN" in _fail_classes(
        without_abandon
    )


def test_eden_guard_bugs_and_fixes() -> None:
    buggy = lm.Scenario(other_eden=1, bugs=frozenset({lm.BUG_EDEN_GUARD}))
    start = lm.initial_states(buggy)[0]
    names = {name for name, _k, _d in lm.successors(start, buggy)}
    assert "CreateBound" not in names
    clean = lm.Scenario(other_eden=1)
    names = {name for name, _k, _d in lm.successors(lm.initial_states(clean)[0], clean)}
    assert "CreateBound" in names

    eden_fixed = lm.explore(lm.Scenario(vcs="eden", fixes=lm.ALL_FIXES))
    assert eden_fixed.liveness_failures() == []


def test_refused_apply_journal_bug() -> None:
    sc = lm.Scenario(bugs=frozenset({lm.BUG_APPLY_JOURNAL}))
    ex = lm.explore(sc)
    assert any(
        name == "RecoverAbsentRefusedApply" and dst.journal == "lifecycle"
        for outs in ex.edges.values()
        for name, _k, dst in outs
    )
    clean = lm.explore(BASELINE)
    assert not any(
        name == "RecoverAbsentRefusedApply"
        for outs in clean.edges.values()
        for name, _k, _d in outs
    )
