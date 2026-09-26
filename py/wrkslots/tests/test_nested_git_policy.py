"""Nested Git metadata in caches: audit predicts exactly what removal permits."""

from __future__ import annotations

import dataclasses
import hashlib
import json
import os
from collections.abc import Callable
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    checkout,
    empty_process_path_census,
    expire_heartbeat,
    make_project,
    prepare_dead_validate_slots,
    stub_validate_batch_censuses,
)


NOT_SELF_CONTAINED = "nested Git metadata that is not self-contained"

# Shaped like a build tool's dependency clone: a remote inside a sibling
# directory whose path happens to contain "worktrees", and a tracking branch.
GENERATED_CONFIG = """[core]
\tbare = false
\trepositoryformatversion = 0
\tfilemode = true
\tlogallrefupdates = true
\tautocrlf = false
[remote "origin"]
\turl = file:///srv/worktrees/validate/checkout/cargo/git/db/dependency-0123
\tfetch = +refs/heads/*:refs/remotes/origin/*
[branch "main"]
\tremote = origin
\tmerge = refs/heads/main
"""


@pytest.fixture(autouse=True)
def isolated_machine_and_cache_home(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "xdg"))


def generated_repository(parent: Path, config: str = GENERATED_CONFIG) -> Path:
    """A self-contained dependency clone: every piece lives below ``parent``."""

    git = parent / ".git"
    for directory in ("objects/info", "objects/pack", "refs/heads", "refs/tags", "info"):
        (git / directory).mkdir(parents=True)
    (git / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
    (git / "refs" / "heads" / "main").write_text("0" * 40 + "\n", encoding="utf-8")
    (git / "config").write_text(config, encoding="utf-8")
    (git / "objects" / "pack" / "pack-0.pack").write_bytes(b"PACK" + b"\0" * 64)
    (parent / "build.sh").write_text("#!/bin/sh\n", encoding="utf-8")
    return git


def classify(parent: Path, policy: str) -> None:
    fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        cli._assert_nested_git_permitted(fd, parent, policy)
    finally:
        os.close(fd)


# --- the classifier ---------------------------------------------------------


def test_refuse_policy_keeps_the_historical_message(tmp_path: Path) -> None:
    generated_repository(tmp_path)
    with pytest.raises(cli.Refusal) as refused:
        classify(tmp_path, cli._NESTED_GIT_REFUSE)
    assert str(refused.value) == (
        f"cache directory contains nested Git metadata: {tmp_path / '.git'}"
    )


def test_default_cache_policy_is_refuse() -> None:
    cache = cli.CacheDirectory(Path("/c/target"), Path("/c"), 1, 2, 3)
    assert cache.nested_git == cli._NESTED_GIT_REFUSE


def test_any_policy_accepts_even_a_linked_administrative_directory(
    tmp_path: Path,
) -> None:
    (tmp_path / ".git").write_text("gitdir: /elsewhere\n", encoding="utf-8")
    classify(tmp_path, cli._NESTED_GIT_ANY)


def test_unknown_policy_is_a_state_error(tmp_path: Path) -> None:
    generated_repository(tmp_path)
    with pytest.raises(cli.StateError, match="unknown nested Git policy"):
        classify(tmp_path, "self_contained")


def test_self_contained_accepts_a_generated_dependency_clone(tmp_path: Path) -> None:
    generated_repository(tmp_path)
    classify(tmp_path, cli._NESTED_GIT_SELF_CONTAINED)


@pytest.mark.parametrize(
    "config",
    [
        "",
        "[core]\n\tbare = false\n\t# worktree = /elsewhere\n",
        "[core]\n\tbare = false\n\t; worktree = /elsewhere\n",
        '[branch "worktree"]\n\tremote = origin\n',
        '[remote "worktree"]\n\turl = /srv/worktree\n',
        "[core]\n\tworktreeish = true\n",
        "[user]\n\tworktree = /elsewhere\n",
        "[include]\n\t# path = /elsewhere\n\t; path = /elsewhere\n",
    ],
    ids=[
        "empty",
        "hash-comment",
        "semicolon-comment",
        "branch",
        "remote",
        "key-prefix",
        "section",
        "include-only-comments",
    ],
)
def test_self_contained_config_reader_names_only_real_links(
    tmp_path: Path, config: str
) -> None:
    generated_repository(tmp_path, config)
    classify(tmp_path, cli._NESTED_GIT_SELF_CONTAINED)


def test_self_contained_accepts_a_repository_without_config_or_objects(
    tmp_path: Path,
) -> None:
    git = tmp_path / ".git"
    git.mkdir()
    (git / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
    classify(tmp_path, cli._NESTED_GIT_SELF_CONTAINED)


def _gitfile(parent: Path) -> None:
    (parent / ".git").write_text("gitdir: /elsewhere/.git/worktrees/x\n", encoding="utf-8")


def _symlinked(parent: Path) -> None:
    elsewhere = parent.parent / f"{parent.name}-elsewhere"
    generated_repository(elsewhere)
    (parent / ".git").symlink_to(elsewhere / ".git")


def _fifo(parent: Path) -> None:
    os.mkfifo(parent / ".git")


def _entry(name: str, *, directory: bool) -> Callable[[Path], None]:
    def make(parent: Path) -> None:
        git = generated_repository(parent)
        if directory:
            (git / name / "linked").mkdir(parents=True)
        else:
            (git / name).write_text("../elsewhere\n", encoding="utf-8")

    return make


def _alternates(parent: Path) -> None:
    git = generated_repository(parent)
    (git / "objects" / "info" / "alternates").write_text(
        "/elsewhere/objects\n", encoding="utf-8"
    )


def _replace_with_symlink(relative: str) -> Callable[[Path], None]:
    def make(parent: Path) -> None:
        git = generated_repository(parent)
        victim = git / relative
        moved = parent.parent / f"{parent.name}-{victim.name}-elsewhere"
        victim.rename(moved)
        victim.symlink_to(moved)

    return make


def _config_fifo(parent: Path) -> None:
    git = generated_repository(parent)
    (git / "config").unlink()
    os.mkfifo(git / "config")


def _config(text: str) -> Callable[[Path], None]:
    def make(parent: Path) -> None:
        generated_repository(parent, text)

    return make


def _oversized_config(parent: Path) -> None:
    padding = "# " + "x" * cli._NESTED_GIT_CONFIG_BYTES_LIMIT + "\n"
    generated_repository(parent, GENERATED_CONFIG + padding)


LINKED: dict[str, tuple[Callable[[Path], None], str]] = {
    "gitfile": (_gitfile, "it is not a real directory"),
    "symlink": (_symlinked, "it is not a real directory"),
    "fifo": (_fifo, "it is not a real directory"),
    "commondir": (_entry("commondir", directory=False), "commondir"),
    "worktrees": (_entry("worktrees", directory=True), "linked worktrees"),
    "modules": (_entry("modules", directory=True), "submodule repositories"),
    "config-worktree": (_entry("config.worktree", directory=False), "per-worktree"),
    "alternates": (_alternates, "alternates"),
    "objects-symlink": (_replace_with_symlink("objects"), "objects is not a real directory"),
    "objects-info-symlink": (
        _replace_with_symlink("objects/info"),
        "info is not a real directory",
    ),
    "config-symlink": (_replace_with_symlink("config"), "cannot be read as a regular file"),
    "config-fifo": (_config_fifo, "config is not a regular file"),
    "config-oversized": (_oversized_config, "exceeds the inspection bound"),
    "core-worktree": (
        _config("[core]\n\tbare = false\n\tworktree = /elsewhere\n"),
        "sets a work tree",
    ),
    "core-worktree-case": (_config("[Core]\nWorkTree=/elsewhere\n"), "sets a work tree"),
    "core-worktree-inline": (_config("[core] worktree = /elsewhere\n"), "sets a work tree"),
    "core-worktree-boolean": (_config("[core]\n\tworktree\n"), "sets a work tree"),
    "core-after-other-section": (
        _config('[remote "origin"]\n\turl = /x\n[core]\n\tworktree = /elsewhere\n'),
        "sets a work tree",
    ),
    "include": (_config("[include]\n\tpath = /elsewhere/config\n"), "includes"),
    "include-if": (
        _config('[includeIf "gitdir:/x/"]\n\tpath = /elsewhere/config\n'),
        "includes",
    ),
    "continuation": (
        _config("[core]\n\tbare = \\\nfalse\n"),
        "sets a work tree or includes",
    ),
    "unterminated-header": (_config("[core\n\tbare = false\n"), "sets a work tree or includes"),
}


@pytest.mark.parametrize("case", sorted(LINKED))
def test_self_contained_refuses_every_link_beyond_the_cache(
    tmp_path: Path, case: str
) -> None:
    make, detail = LINKED[case]
    parent = tmp_path / "dependency"
    parent.mkdir()
    make(parent)
    with pytest.raises(cli.Refusal) as refused:
        classify(parent, cli._NESTED_GIT_SELF_CONTAINED)
    assert NOT_SELF_CONTAINED in str(refused.value)
    assert detail in str(refused.value)


def test_root_key_is_unchanged_for_the_default_policy_and_distinct_otherwise() -> None:
    cache = cli.CacheDirectory(Path("/c/target"), Path("/c"), 1, 2, 3)
    identity = (1, 2, 3, 4, 5)
    historical = hashlib.sha256(
        json.dumps(
            {"path": "/c/target", "checkout_root": "/c", "identity": [1, 2, 3, 4, 5]},
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()
    assert cli._audit_cache_root_key(cache, identity) == historical
    permitted = dataclasses.replace(cache, nested_git=cli._NESTED_GIT_SELF_CONTAINED)
    assert cli._audit_cache_root_key(permitted, identity) != historical


# --- audit and removal agree --------------------------------------------------


def _validation_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[Path, Path]:
    project, _repository, _remote = make_project(tmp_path, cache_globs=("target",))
    prepare_dead_validate_slots(project, ("slot01",))
    expire_heartbeat(project)
    tree = checkout(project, slot="slot01", slot_type="validate")
    dependency = tree / "target" / "install-build" / "dependency"
    dependency.mkdir(parents=True)
    monkeypatch.setattr(cli, "_capture_process_path_census", empty_process_path_census)
    return project, dependency


def _audit_row(
    capsys: pytest.CaptureFixture[str], project: Path
) -> dict[str, object]:
    code = cli.main(
        [
            "--project-root",
            str(project),
            "audit",
            "--format",
            "json",
            "--cache-work-limit",
            "100000",
        ]
    )
    captured = capsys.readouterr()
    assert code == 0, captured.err
    rows = json.loads(captured.out)["slots"]
    assert len(rows) == 1
    row = rows[0]
    assert isinstance(row, dict)
    return row


def _remove_batch(
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
    project: Path,
) -> dict[str, object]:
    stub_validate_batch_censuses(monkeypatch)
    code = cli.main(
        [
            "--project-root",
            str(project),
            "remove-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            "slot01=1",
            "--format",
            "json",
        ]
    )
    captured = capsys.readouterr()
    assert code == 0, captured.err
    report = json.loads(captured.out)
    assert isinstance(report, dict)
    return report


def _expected_cache_bytes(cache: Path) -> int:
    # Independent enumeration, not the census.
    return sum(path.lstat().st_blocks * 512 for path in (cache, *cache.rglob("*")))


def test_validation_audit_and_removal_both_accept_a_self_contained_clone(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, dependency = _validation_slot(tmp_path, monkeypatch)
    generated_repository(dependency)
    cache = dependency.parent.parent

    row = _audit_row(capsys, project)

    assert row["slot_type"] == "validate"
    assert row["verdict"] == "DELETABLE", row["reasons"]
    assert row["reasons"] == []
    assert row["cache_status"] == "complete"
    assert row["cache_error"] is None
    assert row["cache_bytes"] == _expected_cache_bytes(cache)

    report = _remove_batch(capsys, monkeypatch, project)

    assert report["removed"] == [{"generation": 1, "slot": "slot01"}]
    assert report["retained"] == []
    assert not dependency.exists()


@pytest.mark.parametrize("case", ["gitfile", "worktrees", "alternates", "core-worktree"])
def test_validation_audit_and_removal_both_refuse_a_linked_repository(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    case: str,
) -> None:
    project, dependency = _validation_slot(tmp_path, monkeypatch)
    make, detail = LINKED[case]
    make(dependency)
    git = dependency / ".git"
    before = sorted(str(path.relative_to(dependency)) for path in dependency.rglob("*"))

    row = _audit_row(capsys, project)

    assert row["verdict"] == "BLOCKED"
    assert row["cache_status"] == "error"
    assert row["cache_bytes"] is None
    assert NOT_SELF_CONTAINED in str(row["cache_error"])
    assert detail in str(row["cache_error"])
    reasons = row["reasons"]
    assert isinstance(reasons, list)
    assert any(
        str(reason).startswith("cache inspection failed:")
        and NOT_SELF_CONTAINED in str(reason)
        for reason in reasons
    )

    report = _remove_batch(capsys, monkeypatch, project)

    assert report["removed"] == []
    retained = report["retained"]
    assert isinstance(retained, list) and len(retained) == 1
    assert retained[0]["slot"] == "slot01"
    assert NOT_SELF_CONTAINED in retained[0]["reason"]
    assert detail in retained[0]["reason"]
    assert os.path.lexists(git)
    after = sorted(str(path.relative_to(dependency)) for path in dependency.rglob("*"))
    assert after == before
    assert cli._load_active(cli._load_config(str(project), "testhost")).slots


@pytest.mark.parametrize(
    ("allow_git_metadata", "case"),
    [(True, "gitfile"), (True, "alternates"), (False, "self-contained")],
)
def test_removal_refuses_before_deleting_anything(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    allow_git_metadata: bool,
    case: str,
) -> None:
    project, dependency = _validation_slot(tmp_path, monkeypatch)
    if case == "self-contained":
        generated_repository(dependency)
        expected = "cache directory contains nested Git metadata: "
    else:
        LINKED[case][0](dependency)
        expected = NOT_SELF_CONTAINED
    cache = dependency.parent.parent
    # Sorts before install-build, so a destructive pass that checked only on
    # arrival would already have unlinked it.
    earlier = cache / "0-earlier-output"
    earlier.write_text("preserve me\n", encoding="utf-8")
    before = sorted(str(path.relative_to(cache)) for path in cache.rglob("*"))
    tree = cache.parent
    identity = cli._open_directory_identity(tree, "test checkout")

    with pytest.raises(cli.Refusal, match=expected):
        cli._remove_cache_directory(
            cli._load_config(str(project), "testhost"),
            cli.CacheDirectory(
                path=cache,
                checkout_root=tree,
                checkout_device=identity[0],
                checkout_inode=identity[1],
                checkout_mount_id=identity[2],
            ),
            allow_git_metadata=allow_git_metadata,
        )

    assert earlier.read_text(encoding="utf-8") == "preserve me\n"
    assert sorted(str(path.relative_to(cache)) for path in cache.rglob("*")) == before


def test_destructive_pass_refuses_a_link_that_appears_after_the_read_only_pass(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, dependency = _validation_slot(tmp_path, monkeypatch)
    generated_repository(dependency)
    cache = dependency.parent.parent
    tree = cache.parent
    identity = cli._open_directory_identity(tree, "test checkout")
    original = cli._allocated_open_directory
    depth = 0

    def measure_then_link(*args: object, **kwargs: object) -> int:
        nonlocal depth
        depth += 1
        try:
            size: int = original(*args, **kwargs)  # type: ignore[arg-type]
        finally:
            depth -= 1
        if depth == 0:
            (dependency / ".git" / "commondir").write_text("/elsewhere\n", encoding="utf-8")
        return size

    monkeypatch.setattr(cli, "_allocated_open_directory", measure_then_link)

    with pytest.raises(cli.Refusal, match="a commondir names another repository"):
        cli._remove_cache_directory(
            cli._load_config(str(project), "testhost"),
            cli.CacheDirectory(
                path=cache,
                checkout_root=tree,
                checkout_device=identity[0],
                checkout_inode=identity[1],
                checkout_mount_id=identity[2],
            ),
            allow_git_metadata=True,
        )

    assert (dependency / ".git" / "commondir").is_file()
