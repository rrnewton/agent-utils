"""Audit cost regressions (#1794 wrkslots-audit-memory-and-time).

The audit's private-history pre-check must be decisive: it skips the
full scan only when no kept tip can exist, and per-repository Git
facts are enumerated once per repository, not once per row. Handoff
publications folded in one grouped pass must equal the per-record
fold. Temporary fixture registries only.
"""

from __future__ import annotations

import json
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


def test_audit_durable_tips_never_borrow_a_linked_worktrees_refs(tmp_path: Path) -> None:
    """A worktree's refs/worktree/* never become another's negatives (F1)."""

    base = tmp_path / "f1"
    source = base / "S"
    source.mkdir(parents=True)
    _git(source, "init", "-q", "-b", "main")
    (source / "file.txt").write_text("one\n")
    _git(source, "add", "file.txt")
    _git(source, "commit", "-q", "-m", "one")
    holding = base / "P"
    holding.mkdir(parents=True)
    _git(holding, "init", "-q", "-b", "main")
    (holding / "file.txt").write_text("one\n")
    _git(holding, "add", "file.txt")
    _git(holding, "commit", "-q", "-m", "one")
    _git(holding, "checkout", "-q", "-b", "side")
    (holding / "side.txt").write_text("side\n")
    _git(holding, "add", "side.txt")
    _git(holding, "commit", "-q", "-m", "side")
    private_tip = _git(holding, "rev-parse", "HEAD")
    _git(holding, "checkout", "-q", "main")
    # The tip's object lives in the shared store, named only by a
    # per-worktree ref of a linked worktree of the durable repo.
    _git(source, "fetch", "-q", str(holding), "side")
    linked = base / "W2"
    _git(source, "worktree", "add", "-q", str(linked), "HEAD")
    _git(linked, "update-ref", "refs/worktree/keep", private_tip)
    vcs = wrkslots_cli._AuditGitVcs()
    assert private_tip in vcs.durable_ref_tips(linked)
    assert private_tip not in vcs.durable_ref_tips(source)
    kept = vcs.head(holding)
    assert wrkslots_cli._audit_private_history_absent(vcs, holding, kept, (source,)) is False
    full = wrkslots_cli._submodule_private_history(
        wrkslots_cli._GitVcs(), holding, kept, (source,)
    )
    assert full is not None and private_tip in full.tips


def test_audit_precheck_falls_through_on_a_missing_reflog_object(tmp_path: Path) -> None:
    """A missing-object reflog endpoint reaches the full scan, never True (F2)."""

    repo = tmp_path / "cx" / "repo"
    repo.mkdir(parents=True)
    _git(repo, "init", "-q", "-b", "main")
    (repo / "file.txt").write_text("one\n")
    _git(repo, "add", "file.txt")
    _git(repo, "commit", "-q", "-m", "one")
    head = _git(repo, "rev-parse", "HEAD")
    missing = "b" * 40
    old_ts = 1_700_000_000
    logs = repo / ".git" / "logs" / "refs" / "remotes" / "origin"
    logs.mkdir(parents=True)
    (logs / "main").write_text(
        f"{missing} {head} Test <test@example.invalid> {old_ts} +0000\tfetch: storing head\n"
    )
    vcs = wrkslots_cli._AuditGitVcs()
    assert wrkslots_cli._audit_private_history_absent(vcs, repo, head, ()) is False


def test_audit_precheck_true_with_commit_objects_present(tmp_path: Path) -> None:
    """Objects all reachable from the kept HEAD: absent, as the scan finds."""

    repo = tmp_path / "objs" / "repo"
    repo.mkdir(parents=True)
    _git(repo, "init", "-q", "-b", "main")
    for index in range(3):
        (repo / "file.txt").write_text(f"{index}\n")
        _git(repo, "add", "file.txt")
        _git(repo, "commit", "-q", "-m", f"c{index}")
    head = _git(repo, "rev-parse", "HEAD")
    vcs = wrkslots_cli._AuditGitVcs()
    assert vcs.commit_object_ids(repo), "fixture must hold commit objects"
    assert wrkslots_cli._audit_private_history_absent(vcs, repo, head, ()) is True
    assert wrkslots_cli._submodule_private_history(vcs, repo, head, ()) is None


def test_audit_precheck_shared_gitdir_branch(tmp_path: Path) -> None:
    """A linked worktree's own store is judged by the shared branch."""

    repo = tmp_path / "shared" / "repo"
    repo.mkdir(parents=True)
    _git(repo, "init", "-q", "-b", "main")
    (repo / "file.txt").write_text("one\n")
    _git(repo, "add", "file.txt")
    _git(repo, "commit", "-q", "-m", "one")
    linked = tmp_path / "shared" / "linked"
    _git(repo, "worktree", "add", "-q", str(linked), "HEAD")
    vcs = wrkslots_cli._AuditGitVcs()
    kept = vcs.head(linked)
    assert wrkslots_cli._audit_private_history_absent(vcs, linked, kept, ()) is True
    assert wrkslots_cli._submodule_private_history(vcs, linked, kept, ()) is None


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


def test_audit_durable_candidates_shared_per_submodule(tmp_path: Path) -> None:
    """Durable candidates are derived once per (repository, submodule)."""

    root = _nested_submodule_fixture(tmp_path / "nested")
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active(config)
    record = state.slots[0]
    vcs = wrkslots_cli._AuditGitVcs()
    found = wrkslots_cli._submodule_salvage_checkouts(
        config, record.checkouts[0], vcs
    )
    assert found
    submodule, path = found[0]
    authority = vcs.remote_authority(path, submodule.remote)
    first = vcs.durable_candidates(config, submodule, path, authority)
    # A second row's copy of the same submodule reuses the derivation:
    # its path is never even read.
    second = vcs.durable_candidates(
        config, submodule, tmp_path / "no-such-copy", authority
    )
    assert first == second
    assert len(vcs._durable_candidates_cache) == 1


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
    grouped, errors = wrkslots_cli._handoff_write_publications([record], events)
    single = wrkslots_cli._handoff_write_publication(record, events)
    assert errors == {}
    assert grouped[(record.slot, record.generation)] == single
    assert single is not None and single[2] is True


def test_audit_grouped_publications_isolate_a_bad_generation(tmp_path: Path) -> None:
    """One row's bad intent history is its own error, not the audit's (F3)."""

    import dataclasses

    root = _fixture(tmp_path / "case")
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active(config)
    good = state.slots[0]
    bad = dataclasses.replace(good, slot="slot-b", agent="agent-slot-b")
    assert good.owner is not None and bad.owner is not None
    good_artifact: dict[str, object] = {
        "schema": wrkslots_cli.HANDOFF_SIDECAR_SCHEMA,
        "machine": good.machine,
        "slot": good.slot,
        "generation": good.generation,
        "source": "write-handoff",
    }
    bad_artifact: dict[str, object] = dict(good_artifact, schema=999, slot=bad.slot)
    events: list[dict[str, object]] = [
        {
            "kind": "state-imported",
            "sequence": 1,
            "payload": {
                "active": {
                    "slots": [
                        wrkslots_cli._record_to_obj(good),
                        wrkslots_cli._record_to_obj(bad),
                    ]
                },
                "archive": {"records": []},
                "holds": [],
            },
        },
        {
            "kind": "handoff-write-intended",
            "sequence": 2,
            "payload": {
                "slot": good.slot,
                "generation": good.generation,
                "artifact": good_artifact,
                "writer": wrkslots_cli._identity_to_obj(good.owner),
            },
        },
        {
            "kind": "handoff-write-intended",
            "sequence": 3,
            "payload": {
                "slot": bad.slot,
                "generation": bad.generation,
                "artifact": bad_artifact,
                "writer": wrkslots_cli._identity_to_obj(bad.owner),
            },
        },
    ]
    grouped, errors = wrkslots_cli._handoff_write_publications([good, bad], events)
    assert grouped[(good.slot, good.generation)] is not None
    assert (bad.slot, bad.generation) in errors
    with pytest.raises(wrkslots_cli.StateError):
        wrkslots_cli._handoff_write_publication(bad, events)
