"""Audit cost regressions (#1794 wrkslots-audit-memory-and-time).

The audit's private-history pre-check must be decisive: it skips the
full scan only when no kept tip can exist, and per-repository Git
facts are enumerated once per repository, not once per row. Handoff
publications folded in one grouped pass must equal the per-record
fold. Temporary fixture registries only.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_audit_state_rust import _fixture, _git, _nested_submodule_fixture

pytestmark = pytest.mark.usefixtures("pinned_process_evidence")


def _audit_history(root: Path, *, force_full: bool) -> list[tuple[str, ...]]:
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active(config)
    record = state.slots[0]
    checkout = record.checkouts[0]
    vcs = wrkslots_cli._AuditGitVcs()
    found = wrkslots_cli._submodule_salvage_checkouts(config, checkout, vcs)
    assert found, "fixture has no initialized submodule"
    original = wrkslots_cli._audit_private_history_absent
    if force_full:
        wrkslots_cli._audit_private_history_absent = lambda *args: False  # type: ignore[assignment]
    try:
        return [
            _kept_tips(
                wrkslots_cli._audit_submodule_private_history(
                    config, record, submodule, path, vcs
                )
            )
            for submodule, path in found
        ]
    finally:
        wrkslots_cli._audit_private_history_absent = original  # type: ignore[assignment]


def _kept_tips(history: object) -> tuple[str, ...]:
    if history is None:
        return ()
    return tuple(getattr(history, "tips", ()))


def test_audit_private_history_precheck_matches_full_scan_clean(tmp_path: Path) -> None:
    """A clean nested submodule: pre-check and full scan both keep nothing."""

    root = _nested_submodule_fixture(tmp_path / "nested")
    fast = _audit_history(root, force_full=False)
    full = _audit_history(root, force_full=True)
    assert fast == full
    assert any(fast), "fixture must exercise the private case"


def test_audit_private_history_precheck_absent_when_clean(tmp_path: Path) -> None:
    """Without private commits the pre-check itself returns absent."""

    root = _fixture(tmp_path / "case")
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active(config)
    record = state.slots[0]
    vcs = wrkslots_cli._AuditGitVcs()
    # The plain fixture checkout has no submodule; its own repository
    # judged as a submodule store has no private history either.
    checkout_path = wrkslots_cli._stored_path(config, record.checkouts[0].path, "checkout path")
    kept = vcs.head(checkout_path)
    assert wrkslots_cli._audit_private_history_absent(vcs, checkout_path, kept, ()) is True


def test_audit_repository_facts_shared_per_repository(tmp_path: Path) -> None:
    """Durable ref tips are enumerated once per repository (#1794)."""

    root = _fixture(tmp_path / "case")
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active(config)
    record = state.slots[0]
    _relative, repository = wrkslots_cli._stored_repository_path(
        config, record.checkouts[0].repository
    )
    vcs = wrkslots_cli._AuditGitVcs()
    calls: list[str] = []
    original = wrkslots_cli._GitVcs._run

    def counting(repo: Path, args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append(" ".join(str(item) for item in args))  # type: ignore[arg-type]
        return original(repo, args, **kwargs)  # type: ignore[arg-type]

    wrkslots_cli._GitVcs._run = staticmethod(counting)  # type: ignore[assignment]
    try:
        first = vcs.durable_ref_tips(repository)
        second = vcs.durable_ref_tips(repository)
    finally:
        wrkslots_cli._GitVcs._run = original  # type: ignore[assignment]
    assert first == second
    assert sum("for-each-ref" in call for call in calls) == 1


def test_audit_grouped_publications_match_per_record(tmp_path: Path) -> None:
    """One grouped fold equals the per-record publication fold."""

    root = _fixture(tmp_path / "case")
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active(config)
    record = state.slots[0]
    record_obj = wrkslots_cli._record_to_obj(record)
    artifact: dict[str, object] = {
        "schema": wrkslots_cli.HANDOFF_SIDECAR_SCHEMA,
        "machine": record.machine,
        "slot": record.slot,
        "generation": record.generation,
        "source": "write-handoff",
    }
    assert record.owner is not None
    events: list[dict[str, object]] = [
        {
            "kind": "state-imported",
            "sequence": 1,
            "payload": {
                "active": {"slots": [record_obj]},
                "archive": {"records": []},
                "holds": [],
            },
        },
        {
            "kind": "handoff-write-intended",
            "sequence": 2,
            "payload": {
                "slot": record.slot,
                "generation": record.generation,
                "artifact": artifact,
                "writer": wrkslots_cli._identity_to_obj(record.owner),
            },
        },
        {
            "kind": "handoff-write-completed",
            "sequence": 3,
            "payload": {
                "slot": record.slot,
                "generation": record.generation,
                "intent_sequence": 2,
                "artifact_sha256": wrkslots_cli._event_digest(artifact),
            },
        },
    ]
    grouped = wrkslots_cli._handoff_write_publications([record], events)
    single = wrkslots_cli._handoff_write_publication(record, events)
    assert grouped[(record.slot, record.generation)] == single
    assert single is not None and single[2] is True
