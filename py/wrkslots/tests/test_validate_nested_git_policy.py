"""Audit predicts which nested Git repositories completed validation removal deletes.

A completed validation's caches are disposable: `remove --validate-complete`
and `remove-validate-batch` delete a nested repository there whatever it
holds.  They still refuse one whose deletion reaches outside its cache (a
configured work tree elsewhere, a `.git` file or link, linked worktrees,
submodule repositories, included configuration, or a repository Git cannot
open).  Audit judges a validation row's caches with the same judgment, so it
reports `DELETABLE` exactly when that removal would delete, and otherwise the
removal's own refusal.
"""

from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    _nested_clone,
    _slot_removed_evidence,
    active_slots,
    checkout,
    commit_local,
    create,
    git,
    make_project,
    mark_owner_dead_in_current_cgroup,
    raw_command_with_census_authority_stub,
    set_liveness,
)


def _validation_slot(tmp_path: Path) -> tuple[Path, Path, Path]:
    project, _repository, remote = make_project(tmp_path, cache_globs=("target",))
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    return project, checkout(project, slot_type="validate"), remote


def _disposable_repositories(tree: Path, remote: Path) -> tuple[Path, Path]:
    """A clean clone, and one with a local commit and an untracked file."""

    clean = tree / "target" / "deps" / "clean"
    _nested_clone(remote, clean)
    local = tree / "target" / "deps" / "local"
    _nested_clone(remote, local)
    commit_local(local, "local.txt", "local\n", "local only")
    (local / "notes.txt").write_text("draft\n", encoding="utf-8")
    return clean / ".git", local / ".git"


def _reaches_outside(kind: str, tmp_path: Path, remote: Path, repository: Path) -> None:
    """Make ``repository`` (under a validation cache) reach outside the cache."""

    outside = tmp_path / "outside"
    outside.mkdir(exist_ok=True)
    if kind == "malformed":
        (repository / ".git").mkdir(parents=True)
        (repository / ".git" / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
        return
    if kind == "git-file":
        _nested_clone(remote, outside / "elsewhere")
        repository.mkdir(parents=True)
        (repository / ".git").write_text(
            f"gitdir: {outside / 'elsewhere' / '.git'}\n", encoding="utf-8"
        )
        return
    if kind == "git-link":
        _nested_clone(remote, outside / "linked-from")
        repository.mkdir(parents=True)
        (repository / ".git").symlink_to(outside / "linked-from" / ".git")
        return
    _nested_clone(remote, repository)
    if kind == "core-worktree":
        git(repository, "config", "core.worktree", str(outside))
    elif kind == "include":
        (outside / "extra.config").write_text("[core]\n\tworktree = /\n", encoding="utf-8")
        git(repository, "config", "include.path", str(outside / "extra.config"))
    elif kind == "linked-worktree":
        git(repository, "worktree", "add", "-b", "linked", str(outside / "linked"))
    elif kind == "modules":
        modules = repository / ".git" / "modules" / "vendored"
        modules.mkdir(parents=True)
        (modules / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
    else:
        raise AssertionError(kind)


_REACHES_OUTSIDE = (
    ("core-worktree", "repository config sets core.worktree, so its worktree may lie outside this cache"),
    ("include", "repository config sets include.path, which could load configuration this check does not inspect"),
    ("linked-worktree", "repository has registered linked worktrees: linked"),
    ("modules", "repository has submodule repositories under .git/modules: vendored"),
    ("git-file", ".git is a file, not a repository directory (a linked worktree or submodule pointer)"),
    ("git-link", ".git is a symbolic link, not a repository directory"),
    ("malformed", "not a well-formed Git repository: "),
)


def _audit_row(
    project: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> dict[str, object]:
    """Audit in process with process evidence stood in, as the other audit tests do."""

    monkeypatch.setattr(cli, "_assert_slot_unused", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(
        cli, "_capture_process_path_census", lambda _paths: cli._ProcessPathCensus((), ())
    )
    code = cli.main(["--project-root", str(project), "audit", "--format", "json"])
    captured = capsys.readouterr()
    assert code == 0, captured.err
    rows = {row["slot"]: row for row in json.loads(captured.out)["slots"]}
    row = rows["slot01"]
    assert isinstance(row, dict)
    assert row["slot_type"] == "validate"
    return row


def _remove_validate_complete(project: Path) -> tuple[int, str, str]:
    removed = raw_command_with_census_authority_stub(
        project,
        "remove",
        "slot01",
        "--validate-complete",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    return removed.returncode, removed.stdout, removed.stderr


@pytest.mark.parametrize(("kind", "condition"), _REACHES_OUTSIDE, ids=[item[0] for item in _REACHES_OUTSIDE])
def test_audit_and_validation_removal_agree_on_one_tree(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    kind: str,
    condition: str,
) -> None:
    """Same tree, audit then removal: the same refusal; without the offender, both delete."""

    project, tree, remote = _validation_slot(tmp_path)
    clean, local = _disposable_repositories(tree, remote)
    offender = tree / "target" / "deps" / "reaches-outside"
    _reaches_outside(kind, tmp_path, remote, offender)
    mark_owner_dead_in_current_cgroup(project)
    set_liveness(project, "dead")
    refusal = (
        f"cache directory contains nested Git metadata: {offender / '.git'}: "
        f"not provably disposable: {condition}"
    )

    row = _audit_row(project, monkeypatch, capsys)

    assert row["verdict"] == "BLOCKED"
    error = row["cache_error"]
    assert isinstance(error, str) and error.startswith(refusal), error
    assert row["reasons"] == [f"cache inspection failed: {error}"]
    # The disposable repositories are not what blocks the row.
    assert str(clean) not in error and str(local) not in error

    code, _stdout, stderr = _remove_validate_complete(project)

    assert code == 3, stderr
    assert error in stderr
    for dotgit in (clean, local):
        assert dotgit.is_dir()
    assert os.path.lexists(offender / ".git")
    assert active_slots(project)

    shutil.rmtree(offender)
    row = _audit_row(project, monkeypatch, capsys)

    assert row["verdict"] == "DELETABLE", row["reasons"]
    assert row["cache_error"] is None and row["cache_status"] == "complete"
    cache_bytes = row["cache_bytes"]
    assert isinstance(cache_bytes, int) and cache_bytes > 0

    code, _stdout, stderr = _remove_validate_complete(project)

    assert code == 0, stderr
    assert not tree.exists()
    assert active_slots(project) == []
    # The slot-removed event names what was deleted, and that only containment
    # was judged.
    evidence = [cli._as_mapping(item, "test evidence") for item in _slot_removed_evidence(project)]
    assert [item["path"] for item in evidence] == [str(clean), str(local)]
    assert all(item["contents_judged"] is False for item in evidence)
    assert all(item["remote_ref"] is None for item in evidence)


def _judge(dotgit: Path, *, validation_complete: bool) -> cli._DisposableNestedRepository:
    return cli._judge_nested_git_repository(
        dotgit, (dotgit.parent.parent,), validation_complete=validation_complete
    )


@pytest.mark.parametrize(("kind", "condition"), _REACHES_OUTSIDE, ids=[item[0] for item in _REACHES_OUTSIDE])
def test_validation_judgment_refuses_as_the_agent_judgment_does(
    tmp_path: Path, kind: str, condition: str,
) -> None:
    """Each condition that reaches outside the cache refuses in both judgments, alike."""

    remote = _seeded_remote(tmp_path)
    repository = tmp_path / "cache" / "repository"
    _reaches_outside(kind, tmp_path, remote, repository)
    dotgit = repository / ".git"

    refusals: list[str] = []
    for validation_complete in (True, False):
        with pytest.raises(cli.Refusal) as refused:
            _judge(dotgit, validation_complete=validation_complete)
        refusals.append(str(refused.value))

    expected = f"cache directory contains nested Git metadata: {dotgit}: not provably disposable: {condition}"
    assert refusals[0].startswith(expected), refusals[0]
    assert refusals[0] == refusals[1]


def _change_contents(content: str, repository: Path, remote: Path) -> None:
    if content == "local-commit":
        commit_local(repository, "local.txt", "x\n", "local only")
    elif content == "untracked":
        (repository / "notes.txt").write_text("draft\n", encoding="utf-8")
    elif content == "stash":
        (repository / "README").write_text("changed\n", encoding="utf-8")
        git(repository, "stash")
    elif content == "alternates":
        (repository / ".git" / "objects" / "info" / "alternates").write_text(
            f"{remote / 'objects'}\n", encoding="utf-8"
        )
    elif content == "content-filter":
        git(repository, "config", "filter.fixture.clean", "cat")
    elif content == "url-rewrite":
        git(repository, "config", "url.https://example.invalid/.insteadOf", "fixture:")
    elif content == "bare":
        git(repository, "config", "core.bare", "true")
    else:
        raise AssertionError(content)


def _seeded_remote(tmp_path: Path) -> Path:
    remote = tmp_path / "remote.git"
    git(tmp_path, "init", "--bare", "-b", "main", str(remote))
    seed = tmp_path / "seed"
    git(tmp_path, "clone", str(remote), str(seed))
    git(seed, "config", "user.name", "Wrkslots Test")
    git(seed, "config", "user.email", "wrkslots@example.invalid")
    commit_local(seed, "README", "seed\n", "seed")
    git(seed, "push", "origin", "main")
    return remote


@pytest.mark.parametrize(
    "content",
    ("local-commit", "untracked", "stash", "alternates", "content-filter", "url-rewrite", "bare"),
)
def test_validation_judgment_does_not_judge_contents(tmp_path: Path, content: str) -> None:
    """What only the content proof needs is not asked of a completed validation's cache."""

    remote = _seeded_remote(tmp_path)
    repository = tmp_path / "cache" / "repository"
    _nested_clone(remote, repository)
    _change_contents(content, repository, remote)
    dotgit = repository / ".git"

    with pytest.raises(cli.Refusal, match="not provably disposable"):
        _judge(dotgit, validation_complete=False)
    judged = _judge(dotgit, validation_complete=True)

    assert judged.path == dotgit
    assert judged.contents_judged is False
    assert judged.remote_ref is None
    metadata = os.lstat(dotgit)
    assert judged.identity == (metadata.st_dev, metadata.st_ino)
    assert judged.head == git(repository, "rev-parse", "HEAD").stdout.strip()


def _cache(tmp_path: Path, *, nested_git: str) -> cli.CacheDirectory:
    checkout_root = cli_config(tmp_path).root / "checkout"
    identity = cli._open_directory_identity(checkout_root, "checkout")
    return cli.CacheDirectory(checkout_root / "target", checkout_root, *identity, nested_git=nested_git)


def cli_config(tmp_path: Path) -> cli.Config:
    from wrkslots.tests.test_census_cost import _config

    return _config(tmp_path)


def _census(tmp_path: Path, cache: cli.CacheDirectory, name: str) -> cli._AuditCacheMeasurement:
    measured, _work = cli._audit_cache_census(
        cli_config(tmp_path),
        (cli.ActiveState("testhost", 31, ()),),
        {"subject": (cache,)},
        {},
        state_path=tmp_path / f"{name}.json",
        work_limit=10_000,
        wall_seconds=30,
    )
    return measured["subject"]


def test_audit_cache_census_judges_a_validation_cache_and_measures_its_repositories(
    tmp_path: Path,
) -> None:
    remote = _seeded_remote(tmp_path)
    clone = cli_config(tmp_path).root / "checkout" / "target" / "deps" / "clone"
    _nested_clone(remote, clone)
    commit_local(clone, "local.txt", "x\n", "local only")

    refused = _census(tmp_path, _cache(tmp_path, nested_git=cli._NESTED_GIT_REFUSE), "refuse")
    judged = _census(
        tmp_path, _cache(tmp_path, nested_git=cli._NESTED_GIT_VALIDATION_COMPLETE), "validation"
    )

    assert refused.status == "error"
    assert refused.error == f"cache directory contains nested Git metadata: {clone / '.git'}"
    assert judged.status == "complete" and judged.error is None
    assert judged.bytes is not None and judged.bytes > 0

    git(clone, "config", "core.worktree", str(tmp_path))
    blocked = _census(
        tmp_path, _cache(tmp_path, nested_git=cli._NESTED_GIT_VALIDATION_COMPLETE), "validation"
    )

    assert blocked.status == "error"
    assert blocked.error == (
        f"cache directory contains nested Git metadata: {clone / '.git'}: not provably "
        "disposable: repository config sets core.worktree, so its worktree may lie "
        "outside this cache"
    )


def test_audit_cache_root_keys_differ_by_nested_git_policy(tmp_path: Path) -> None:
    (cli_config(tmp_path).root / "checkout" / "target").mkdir(parents=True)
    identity = (1, 2, 3, 4, 5)
    refuse = _cache(tmp_path, nested_git=cli._NESTED_GIT_REFUSE)
    validation = _cache(tmp_path, nested_git=cli._NESTED_GIT_VALIDATION_COMPLETE)
    default = cli.CacheDirectory(
        refuse.path,
        refuse.checkout_root,
        refuse.checkout_device,
        refuse.checkout_inode,
        refuse.checkout_mount_id,
    )

    # The default policy keeps the key it always had.
    assert cli._audit_cache_root_key(refuse, identity) == cli._audit_cache_root_key(
        default, identity
    )
    assert cli._audit_cache_root_key(refuse, identity) != cli._audit_cache_root_key(
        validation, identity
    )


def test_journaled_evidence_accepts_only_a_false_contents_judgment() -> None:
    item: dict[str, object] = {
        "checkout": "product",
        "path": "/slot/product/target/deps/clean/.git",
        "head": None,
        "remote_ref": None,
        "ignored_entries": 0,
        "ignored_sample": [],
    }

    assert cli._nested_git_evidence_from_obj([item], "test") == [item]
    unjudged = {**item, "contents_judged": False}
    assert cli._nested_git_evidence_from_obj([unjudged], "test") == [unjudged]
    with pytest.raises(cli.StateError, match="contents_judged must be false"):
        cli._nested_git_evidence_from_obj([{**item, "contents_judged": True}], "test")
