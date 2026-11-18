"""Checked transition-model tests (#1828).

The model in ``wrkslots.transitionmodel`` transcribes each
command's phase structure from cli.py generically (lock-free read,
lock, the checks exactly as written, append, snapshot write,
crash points between).  These tests are the oracle, not an input:
the forked revision chain of #1807 and the deleted-while-locked
checkout of #1766 must be *found* by the search on the code
variants that contain them and must vanish under the variants
that fix them.  Candidate wedges are confirmed or refuted the
same way; see ai_docs/reports/research/
2026-11-18-wrkslots-model-check.md for the full reading.

Cost split (owner rule: fast, high-signal gating tests).  The
default run is the fast subset (target <= ~20 s): both
acceptance oracles, fix closure, the C1 wedge shapes, happy
paths, and a one-step smoke over every battery scenario.  The
full exploration battery (remaining scenarios and candidates)
is opt-in for a developer run by hand:

    WRKSLOTS_MODEL_FULL=1 python3 -m pytest \
        py/wrkslots/tests/test_transition_model.py -q

or, without assertions, ``python3 py/wrkslots/transitionmodel.py``.
"""

from __future__ import annotations

import os

import pytest

from wrkslots import transitionmodel as tm

FULL = pytest.mark.skipif(
    os.environ.get("WRKSLOTS_MODEL_FULL") != "1",
    reason="full model battery is opt-in (WRKSLOTS_MODEL_FULL=1)",
)

CURRENT = tm.Scenario()
CURRENT_FOREIGN = tm.Scenario(reg_a="foreign")
PREFIX_OWN = tm.Scenario(fixes=frozenset(), reg_a="own")
PREFIX_FOREIGN = tm.Scenario(fixes=frozenset(), reg_a="foreign")
ALLFIX_OWN = tm.Scenario(fixes=tm.ALL_FIXES, reg_a="own")
ALLFIX_FOREIGN = tm.Scenario(fixes=tm.ALL_FIXES, reg_a="foreign")


@pytest.fixture(scope="module")
def current() -> tm.Exploration:
    return tm.explore(CURRENT)


@pytest.fixture(scope="module")
def prefix_foreign() -> tm.Exploration:
    return tm.explore(PREFIX_FOREIGN)


@pytest.fixture(scope="module")
def allfix_own() -> tm.Exploration:
    return tm.explore(ALLFIX_OWN)


@pytest.fixture(scope="module")
def current_foreign() -> tm.Exploration:
    return tm.explore(CURRENT_FOREIGN)


@pytest.fixture(scope="module")
def prefix_own() -> tm.Exploration:
    return tm.explore(PREFIX_OWN)


@pytest.fixture(scope="module")
def allfix_foreign() -> tm.Exploration:
    return tm.explore(ALLFIX_FOREIGN)


@pytest.fixture(scope="module")
def create_run() -> tm.Exploration:
    return tm.explore(tm.Scenario(b_present=False))


@pytest.fixture(scope="module")
def misjudged() -> tm.Exploration:
    return tm.explore(tm.Scenario(misjudged_a=True))


@pytest.fixture(scope="module")
def contention() -> tm.Exploration:
    return tm.explore(
        tm.Scenario(
            contention=True,
            b_unbound=True,
            max_heartbeats=4,
            disable=frozenset({"rm", "cr", "vb"}),
        )
    )


def _resting(ex: tm.Exploration, states: list[tm.State]) -> list[tm.State]:
    return [s for s in states if tm.is_resting(s)]


def _crash_free(ex: tm.Exploration, states: list[tm.State]) -> list[tm.State]:
    return [
        s for s in states
        if not any("Crash" in step for step in ex.trace(s))
    ]


# --- Fast gating subset: acceptance oracles and smoke ------------

def test_1807_fork_found_on_current_main(current: tm.Exploration) -> None:
    forks = current.violations("forked-chain")
    assert forks
    plain = _crash_free(current, forks)
    assert plain, "the fork needs no crash, only an interleaving"
    best = current.shortest(plain)
    assert best is not None
    trace = current.trace(best)
    # The interleaving the production report describes, in order:
    # lock-free load, another slot's append inside the salvage
    # window, identity captured afterwards, assert passes, stale
    # append forks the chain.
    order = [
        "RmLoadFree",
        "HbAppend",
        "RmCaptureIdentity",
        "RmAssertCurrent",
        "RmAppendBasis",
    ]
    idx = [trace.index(name) for name in order]
    assert idx == sorted(idx)
    assert best.ok == 0


def test_1807_snapshot_rolls_other_slot_back(
    current: tm.Exploration,
) -> None:
    rolled = [
        s
        for s in _crash_free(current, current.violations("forked-chain"))
        if "RmWriteActive" in current.trace(s)
        and s.snap_hb[tm.B] < s.log_hb[tm.B]
    ]
    assert rolled, "ACTIVE must be writable from the stale registry"


def test_1807_append_after_capture_always_defers(
    current: tm.Exploration,
) -> None:
    # Discriminating direction: no fork trace has the other
    # slot's append land after the identity capture, and the
    # deferral edge the code takes instead is reachable.
    for s in current.violations("forked-chain"):
        trace = current.trace(s)
        if "HbAppend" in trace and "RmCaptureIdentity" in trace:
            assert trace.index("HbAppend") < trace.index(
                "RmCaptureIdentity"
            ), trace
    assert any(
        name == "RmDeferStale"
        for outs in current.edges.values()
        for name, _kind, _dst in outs
    )


def test_fix_variants_close_both_defects(
    allfix_own: tm.Exploration,
) -> None:
    assert allfix_own.violations() == []
    assert allfix_own.wedges() == []
    defence_only = tm.explore(
        tm.Scenario(fixes=tm.CURRENT_MAIN | {tm.STALEREF})
    )
    assert defence_only.violations() == []
    assert defence_only.wedges() == []


def test_1766_prefix_deletes_while_locked(
    prefix_foreign: tm.Exploration, current: tm.Exploration
) -> None:
    hits = _resting(
        prefix_foreign, prefix_foreign.violations("deleted-while-locked")
    )
    assert hits
    assert any(s.storage[tm.A] == "deleted" for s in hits)
    assert current.violations("deleted-while-locked") == []


def test_fork_with_journal_or_seal_is_a_true_wedge(
    current: tm.Exploration,
) -> None:
    wedges = _resting(current, current.wedges())
    assert wedges
    # Fork buried under the removal's own later events, with its
    # finish journal outstanding: recover replays and refuses,
    # tail recovery refuses on the journal.
    assert any(
        s.ok == 0 and s.fork_tip == 0
        and s.jrnl[tm.A] in ("fpend", "ffence")
        for s in wedges
    )
    # Fork still at the tip, no journal, but a dead validation
    # seal: tail recovery refuses on partial state, recover
    # refuses on replay.
    assert any(
        s.ok == 0 and s.fork_tip == 1
        and s.jrnl == ("none", "none") and s.seal == "dead"
        for s in wedges
    )
    # And every wedge in this scenario is a fork descendant:
    # nothing wedges on a loadable chain.
    assert all(s.ok == 0 for s in current.wedges())


def test_bare_fork_heals_via_tail_recovery(
    current: tm.Exploration,
) -> None:
    # A fork at the tip with nothing else outstanding is what
    # recover-tail-event exists for: some forked history rests
    # clean afterwards (the ghost keeps the testimony).
    healed = [
        s for s in current.states
        if tm.is_clean(s) and s.viol == "forked-chain"
    ]
    assert healed
    assert any(
        "RecTailTruncate" in current.trace(s) for s in healed
    )


def test_happy_path_archives_clean(current: tm.Exploration) -> None:
    archived_a = [
        s for s in current.states if tm.is_clean(s) and s.arch[tm.A] == 1
    ]
    assert archived_a
    trace = current.trace(archived_a[0])
    assert trace and all(isinstance(step, str) for step in trace)


def test_every_battery_scenario_steps_cleanly() -> None:
    # Smoke, not search: every battery configuration's start
    # state must step without error for two plies.  Full
    # explorations of these scenarios are the opt-in battery.
    scenarios = [
        CURRENT,
        CURRENT_FOREIGN,
        PREFIX_OWN,
        PREFIX_FOREIGN,
        ALLFIX_OWN,
        ALLFIX_FOREIGN,
        tm.Scenario(fixes=tm.CURRENT_MAIN | {tm.REVCHECK}),
        tm.Scenario(retained_writer=True),
        tm.Scenario(b_present=False),
        tm.Scenario(misjudged_a=True),
        tm.Scenario(
            contention=True,
            b_unbound=True,
            max_heartbeats=4,
            disable=frozenset({"rm", "cr", "vb"}),
        ),
    ]
    for sc in scenarios:
        start = tm.initial_states(sc)[0]
        first = tm.successors(start, sc)
        assert first, sc
        for _name, _kind, nxt in first:
            assert tm.successors(nxt, sc) is not None


# --- Full battery (opt-in): remaining scenarios and candidates ---

@FULL
@pytest.mark.parametrize(
    "scenario",
    [
        tm.Scenario(fixes=tm.CURRENT_MAIN | {tm.REVCHECK}),
        tm.Scenario(fixes=tm.CURRENT_MAIN | {tm.REBASE}),
        tm.Scenario(retained_writer=True),
    ],
    ids=["revcheck", "rebase", "retained-writer"],
)
def test_1807_each_remaining_fix_closes_the_fork(
    scenario: tm.Scenario,
) -> None:
    ex = tm.explore(scenario)
    assert ex.violations() == []
    assert ex.wedges() == []


@FULL
def test_1766_prefix_own_deletes_while_locked(
    prefix_own: tm.Exploration, allfix_foreign: tm.Exploration
) -> None:
    hits = _resting(prefix_own, prefix_own.violations("deleted-while-locked"))
    assert hits
    assert any(s.storage[tm.A] == "deleted" for s in hits)
    assert allfix_foreign.violations("deleted-while-locked") == []


@FULL
def test_1766_foreign_lock_is_manual_with_storage_intact(
    current_foreign: tm.Exploration, prefix_foreign: tm.Exploration
) -> None:
    # Fixed: a foreign lock strands the batch before anything is
    # deleted; the only exit is the documented hand unlock.
    manual = _resting(
        current_foreign, current_foreign.manual_terminals()
    )
    assert manual
    assert all(s.storage[tm.A] != "deleted" for s in manual)
    # Pre-fix: the same stranded states exist with the subvolume
    # already deleted underneath them.
    stranded = _resting(
        prefix_foreign,
        prefix_foreign.manual_terminals() + prefix_foreign.wedges(),
    )
    assert any(s.storage[tm.A] == "deleted" for s in stranded)


@FULL
def test_c2_config_ttl_edit_strands_create_recovery(
    create_run: tm.Exploration,
) -> None:
    stranded = _resting(
        create_run,
        [
            s
            for s in create_run.manual_terminals()
            if s.cfg_ttl == 1
            and any(j.startswith("create") for j in s.jrnl)
        ],
    )
    assert stranded, "recover refuses; only reverting config exits"
    best = create_run.shortest(stranded)
    assert best is not None
    assert create_run.trace(best)[-1] == "ConfigTtlEdit"
    # It is a refusal loop between verbs, not chain damage.
    assert all(s.ok == 0 for s in create_run.wedges())


@FULL
def test_c3_renewed_heartbeat_delays_but_never_wedges(
    misjudged: tm.Exploration,
) -> None:
    renewed = [
        s
        for s in misjudged.states
        if s.ok == 1
        and s.jrnl[tm.A] in tm._FINISH_KINDS
        and s.jlive[tm.A] == 0
        and s.hb_exp[tm.A] == 0
    ]
    assert renewed, "the re-gated state itself must be reachable"
    stuck = {s for s in misjudged.dead_journals() if s.ok == 1}
    assert not ({*renewed} & stuck)


@FULL
def test_c4_hold_and_finish_journal_never_coexist(
    current: tm.Exploration, misjudged: tm.Exploration,
    create_run: tm.Exploration,
) -> None:
    for ex in (current, misjudged, create_run):
        assert not [
            s
            for s in ex.states
            if s.hold[tm.A] == 1 and s.jrnl[tm.A] in tm._FINISH_KINDS
        ]


@FULL
def test_c5_snapshot_divergence_heals_without_a_fork(
    current: tm.Exploration,
) -> None:
    divergent = [
        s
        for s in current.states
        if s.ok == 1
        and (s.snap_rev, s.snap_hb, s.snap_pres)
        != (s.tip, s.log_hb, s.log_pres)
    ]
    assert divergent, "crash between append and snapshot must show"
    clean_reachable = current._reaching_clean(None)
    assert all(s in clean_reachable for s in divergent)


@FULL
def test_contention_retry_budget_can_be_exhausted(
    contention: tm.Exploration,
) -> None:
    busy = [s for s in contention.states if s.actors[4].pc == tm.BUSY]
    assert busy, "sustained churn must be able to starve the adopter"
    committed = [
        s for s in contention.states if s.actors[4].pc == tm.DONE
    ]
    assert committed, "but the commit is reachable, not impossible"
    assert contention.violations() == []
    assert contention.wedges() == []


@FULL
def test_create_happy_path(create_run: tm.Exploration) -> None:
    created_b = [
        s
        for s in create_run.states
        if tm.is_clean(s)
        and s.log_pres[tm.B] == 1
        and s.actors[2].pc == tm.DONE
    ]
    assert created_b
