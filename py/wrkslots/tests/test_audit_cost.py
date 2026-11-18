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
from collections.abc import Iterable
from pathlib import Path
from typing import cast

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
        wrkslots_cli._audit_private_history_absent = lambda *args: False
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
        wrkslots_cli._audit_private_history_absent = original


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
    """Durable candidates are derived once for one submodule checkout."""

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
    # The same checkout reuses the derivation; another checkout has
    # its own user-config context and is never answered from it.
    second = vcs.durable_candidates(config, submodule, path, authority)
    assert first == second
    assert len(vcs._durable_candidates_cache) == 1


def test_audit_repository_facts_shared_per_repository(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
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
        calls.append(" ".join(str(item) for item in cast(Iterable[object], args)))
        return original(repo, args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(
        wrkslots_cli._GitVcs, "_run", staticmethod(counting)
    )
    first = vcs.durable_ref_tips(repository)
    second = vcs.durable_ref_tips(repository)
    assert first == second
    assert sum("for-each-ref" in call for call in calls) == 1


def _remote_authority_outcome(
    vcs: wrkslots_cli._GitVcs, checkout: Path
) -> tuple[str, str]:
    try:
        return "url", vcs.remote_authority(checkout, "origin").url
    except wrkslots_cli.Refusal as exc:
        return "refused", str(exc)


def _conditional_remote_repo(
    base: Path, condition: str, extra: str
) -> tuple[Path, Path]:
    """A repository whose two linked worktrees see different remotes."""

    remote = base / "remote.git"
    remote.mkdir(parents=True)
    _git(remote, "init", "-q", "--bare", "-b", "main")
    main = base / "main"
    main.mkdir(parents=True)
    _git(main, "init", "-q", "-b", "main")
    (main / "file.txt").write_text("one\n")
    _git(main, "add", "file.txt")
    _git(main, "commit", "-q", "-m", "one")
    _git(main, "remote", "add", "origin", str(remote))
    _git(main, "branch", "b1")
    _git(main, "branch", "b2")
    w1 = base / "w1"
    w2 = base / "w2"
    _git(main, "worktree", "add", "-q", str(w1), "b1")
    _git(main, "worktree", "add", "-q", str(w2), "b2")
    if condition == "onbranch":
        included = main / ".git" / "extra-onbranch.cfg"
        included.write_text(extra)
        config = main / ".git" / "config"
        config.write_text(
            config.read_text()
            + '\n[includeIf "onbranch:b2"]\n\tpath = extra-onbranch.cfg\n'
        )
    else:
        included = main / ".git" / "extra-gitdir.cfg"
        included.write_text(extra)
        gitdir = main / ".git" / "worktrees" / "w2"
        config = main / ".git" / "config"
        config.write_text(
            config.read_text()
            + f'\n[includeIf "gitdir:{gitdir}"]\n\tpath = extra-gitdir.cfg\n'
        )
    return w1, w2


def test_audit_remote_authority_never_shares_an_onbranch_answer(
    tmp_path: Path,
) -> None:
    """An onbranch: pushurl belongs to its own worktree only (review q1)."""

    w1, w2 = _conditional_remote_repo(
        tmp_path / "q1",
        "onbranch",
        '[remote "origin"]\n\tpushurl = /tmp/elsewhere.git\n',
    )
    plain = wrkslots_cli._GitVcs()
    expected = _remote_authority_outcome(plain, w2)
    audit = wrkslots_cli._AuditGitVcs()
    assert _remote_authority_outcome(audit, w1)[0] == "url"
    assert _remote_authority_outcome(audit, w2) == expected
    assert expected[0] == "refused"


def test_audit_remote_authority_never_shares_a_gitdir_answer(
    tmp_path: Path,
) -> None:
    """A gitdir: insteadOf rewrite belongs to its own worktree only (q1g)."""

    base = tmp_path / "q1g"
    _conditional_remote_repo(
        base,
        "gitdir",
        '[url "/tmp/rewritten.git"]\n'
        f"\tinsteadOf = {base / 'remote.git'}\n",
    )
    plain = wrkslots_cli._GitVcs()
    expected = _remote_authority_outcome(plain, base / "w2")
    audit = wrkslots_cli._AuditGitVcs()
    _remote_authority_outcome(audit, base / "w1")
    assert _remote_authority_outcome(audit, base / "w2") == expected


def test_audit_effective_urls_never_cross_a_worktree_config(
    tmp_path: Path,
) -> None:
    """A config.worktree insteadOf is not another worktree's answer (q2)."""

    base = tmp_path / "q2"
    mirror_a = base / "mirrorA.git"
    mirror_b = base / "mirrorB.git"
    for mirror in (mirror_a, mirror_b):
        mirror.mkdir(parents=True)
        _git(mirror, "init", "-q", "--bare", "-b", "main")
    module = base / "mod"
    module.mkdir(parents=True)
    _git(module, "init", "-q", "-b", "main")
    (module / "file.txt").write_text("one\n")
    _git(module, "add", "file.txt")
    _git(module, "commit", "-q", "-m", "one")
    _git(module, "config", "extensions.worktreeConfig", "true")
    _git(module, "remote", "add", "origin", str(mirror_b))
    w1 = base / "w1"
    w2 = base / "w2"
    _git(module, "worktree", "add", "-q", str(w1), "HEAD")
    _git(module, "worktree", "add", "-q", str(w2), "HEAD")
    (module / ".git" / "worktrees" / "w1" / "config.worktree").write_text(
        f'[url "{mirror_a}"]\n\tinsteadOf = {mirror_b}\n'
    )
    (module / ".git" / "worktrees" / "w2" / "config.worktree").write_text(
        '[user]\n\tname = x\n'
    )
    audit = wrkslots_cli._AuditGitVcs()
    assert audit.remote_effective_urls(w1, "origin") == (str(mirror_a),)
    assert audit.remote_effective_urls(w2, "origin") == (str(mirror_b),)
    assert wrkslots_cli._GitVcs().remote_effective_urls(w2, "origin") == (
        str(mirror_b),
    )
    # The durable candidates built from those URLs differ too.
    root = _fixture(base / "case")
    config = wrkslots_cli._load_config(str(root), None)
    source = root / "src.git"
    source.mkdir()
    _git(source, "init", "-q", "--bare", "-b", "main")
    for checkout_path in (w1, w2):
        authority = audit.remote_authority(checkout_path, "origin")
        submodule = wrkslots_cli.Checkout(
            name="mod",
            path="mod",
            repository="src.git",
            branch="",
            start_point=_git(checkout_path, "rev-parse", "HEAD"),
            remote="origin",
            remote_url_sha256=authority.sha256,
            landed_ref="refs/remotes/origin/main",
            head=_git(checkout_path, "rev-parse", "HEAD"),
        )
        _source, mirrors = audit.durable_candidates(
            config, submodule, checkout_path, authority
        )
        assert mirrors == ((mirror_a,) if checkout_path == w1 else (mirror_b,))


def test_audit_durable_candidates_never_cross_a_global_gitdir_include(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A global gitdir: insteadOf for one clone is not another's (q3)."""

    base = tmp_path / "q3"
    root = _fixture(base / "case")
    config = wrkslots_cli._load_config(str(root), None)
    origin = root / "origin.git"
    mirror_a = root / "mirrorA.git"
    source = root / "src.git"
    for repository in (origin, mirror_a, source):
        repository.mkdir()
        _git(repository, "init", "-q", "--bare", "-b", "main")
    clones: list[Path] = []
    for name in ("c1", "c2"):
        clone = root / name
        clone.mkdir()
        _git(clone, "init", "-q", "-b", "main")
        (clone / "file.txt").write_text("one\n")
        _git(clone, "add", "file.txt")
        _git(clone, "commit", "-q", "-m", "one")
        _git(clone, "remote", "add", "origin", str(origin))
        clones.append(clone)
    included = root / "inc.cfg"
    included.write_text(f'[url "{mirror_a}"]\n\tinsteadOf = {origin}\n')
    global_config = root / "gcfg"
    global_config.write_text(
        f'[includeIf "gitdir:{clones[0]}/"]\n\tpath = {included}\n'
    )
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(global_config))
    monkeypatch.setenv("GIT_CONFIG_SYSTEM", "/dev/null")
    audit = wrkslots_cli._AuditGitVcs()
    results: list[tuple[Path, ...]] = []
    for clone in clones:
        authority = audit.remote_authority(clone, "origin")
        submodule = wrkslots_cli.Checkout(
            name="mod",
            path="mod",
            repository="src.git",
            branch="",
            start_point=_git(clone, "rev-parse", "HEAD"),
            remote="origin",
            remote_url_sha256=authority.sha256,
            landed_ref="refs/remotes/origin/main",
            head=_git(clone, "rev-parse", "HEAD"),
        )
        results.append(
            audit.durable_candidates(config, submodule, clone, authority)[1]
        )
    assert mirror_a in results[0]
    assert mirror_a not in results[1]
    plain_authority = wrkslots_cli._GitVcs().remote_authority(clones[1], "origin")
    plain_submodule = wrkslots_cli.Checkout(
        name="mod",
        path="mod",
        repository="src.git",
        branch="",
        start_point=_git(clones[1], "rev-parse", "HEAD"),
        remote="origin",
        remote_url_sha256=plain_authority.sha256,
        landed_ref="refs/remotes/origin/main",
        head=_git(clones[1], "rev-parse", "HEAD"),
    )
    assert results[1] == wrkslots_cli._durable_candidates(
        wrkslots_cli._GitVcs(), config, plain_submodule, clones[1], plain_authority
    )[1]


def test_streamed_replay_matches_list_replay(tmp_path: Path) -> None:
    """A bounded scan's streamed fold derives the list fold's state."""

    root = _fixture(tmp_path / "case")
    config = wrkslots_cli._load_config(str(root), None)
    active_doc = json.loads(
        (root / "worktrees" / "ACTIVE.node-a.json").read_text()
    )
    archive_doc = json.loads(
        (root / "worktrees" / "ARCHIVED.node-a.json").read_text()
    )
    with wrkslots_cli._event_memo_scope():
        writer = wrkslots_cli._event_writer(config, "node-a")
        writer.append(
            "state-imported",
            {"active": active_doc, "archive": archive_doc, "holds": []},
        )
        listed = wrkslots_cli._states_from_events(
            config, "node-a", events=wrkslots_cli._load_events(config, "node-a")
        )
    with wrkslots_cli._event_memo_scope():
        wrkslots_cli._bound_event_retention()
        streamed = wrkslots_cli._states_from_events(config, "node-a")
        # A second derivation in the same bounded scope resumes from
        # the remembered fold and derives the same state.
        resumed = wrkslots_cli._states_from_events(config, "node-a")
    assert listed is not None and streamed is not None
    assert streamed == listed
    assert resumed == listed


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
