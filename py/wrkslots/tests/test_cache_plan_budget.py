"""Audit cache-glob planning is bounded, fair and faithful to ``Path.glob``."""

from __future__ import annotations

import contextlib
import dataclasses
import json
import os
import subprocess
import types
from collections.abc import Iterator
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_census_cost import _config

LIVE_SHAPED = (
    "node_modules",
    "target",
    "ignored/**/target",
    "ignored/**/target-debug",
    "ignored/**/components-target",
)


def _tree(tmp_path: Path) -> Path:
    root = tmp_path / "checkout"
    outside = tmp_path / "outside"
    (outside / "target").mkdir(parents=True)
    for directory in (
        "target",
        "ignored/a/target/nested/target",
        "ignored/a/b/c/target-debug",
        "ignored/a/bb/target",
        "ignored/.hidden/components-target",
        "ignored/tfile",
        "ignored/slink",
        "ignored/broken",
        "ignored/deep/b/x/y/target-debug",
        "a/bb/d",
        "a/bc/d",
        "a/bcd/d",
        "a/x/d",
    ):
        (root / directory).mkdir(parents=True)
    (root / "ignored/a/target/deep-file").write_bytes(b"x")
    (root / "ignored/a/target/nested/target-debug").write_bytes(b"x")
    (root / "ignored/tfile/target").write_bytes(b"x")
    (root / "ignored/slink/target").symlink_to(outside / "target", target_is_directory=True)
    (root / "ignored/broken/target").symlink_to(root / "absent")
    (root / "ignored/link").symlink_to(outside, target_is_directory=True)
    (root / "node_modules").symlink_to(root / "target", target_is_directory=True)
    return root


def _expand(root: Path, pattern: str) -> list[Path]:
    ((expanded_pattern, found),) = cli._expand_cache_globs(root, (pattern,))
    assert expanded_pattern == pattern
    return list(found)


@pytest.mark.parametrize(
    "pattern",
    (
        *LIVE_SHAPED,
        "ignored/*/target",
        "ignored/link/target",
        "ignored/**/*/target",
        "ignored/*/**/target",
        "ignored/**/b/**/target-debug",
        "ignored/**/**/target",
        "a/b?/d",
        "a/[bc]*/d",
        "a/*/d",
        "missing/**/target",
    ),
)
def test_expansion_yields_what_path_glob_yields(tmp_path: Path, pattern: str) -> None:
    root = _tree(tmp_path)
    expected = list(root.glob(pattern))
    found = _expand(root, pattern)
    assert sorted(found) == sorted(expected)
    assert len(found) == len(set(found))
    parts = pattern.split("/")
    if "**" not in parts or (
        parts.count("**") == 1
        and parts[-2] == "**"
        and not any(magic in "".join(parts[:-2]) for magic in "*?[")
    ):
        # Only these shapes are also yielded in Path.glob's order, so the first
        # refusal a checkout reports is unchanged. Other shapes, such as
        # 'ignored/*/**/target', may name a different first unsafe path.
        assert found == expected


def test_trailing_recursive_wildcard_yields_directories_without_following_links(
    tmp_path: Path,
) -> None:
    # Python 3.13 also yields files here; the 3.12 behavior is the contract.
    root = _tree(tmp_path)
    expected = [
        Path(directory)
        for directory, _names, _files in os.walk(root / "ignored", followlinks=False)
    ]
    assert sorted(_expand(root, "ignored/**")) == sorted(expected)


def test_unreadable_directory_contributes_nothing(tmp_path: Path) -> None:
    root = _tree(tmp_path)
    closed = root / "ignored/a"
    closed.chmod(0)
    try:
        if os.access(closed, os.R_OK):
            pytest.skip("directory permissions are not enforced for this user")
        for pattern in LIVE_SHAPED:
            expected = list(root.glob(pattern))
            assert _expand(root, pattern) == expected
        assert not any(
            closed in path.parents for path in _expand(root, "ignored/**/target")
        )
    finally:
        closed.chmod(0o755)


@contextlib.contextmanager
def _counted_listings(monkeypatch: pytest.MonkeyPatch) -> Iterator[list[str]]:
    listed: list[str] = []
    real = os.scandir

    def scandir(path: str | os.PathLike[str]) -> Iterator[os.DirEntry[str]]:
        listed.append(os.fspath(path))
        return real(path)

    with monkeypatch.context() as patch:
        patch.setattr(os, "scandir", scandir)
        yield listed


def test_joint_expansion_lists_each_directory_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _tree(tmp_path)
    budget = cli._CachePlanBudget(
        cli._AuditWorkBudget(1_000), 3_600.0, 3_600.0
    )
    with _counted_listings(monkeypatch) as listed:
        expanded = cli._expand_cache_globs(root, LIVE_SHAPED, budget)
    # The checkout root, then every directory below ignored/ reached without
    # following a symlink, including the insides of matched directories.
    below = [
        directory
        for directory, _names, _files in os.walk(root / "ignored", followlinks=False)
    ]
    assert sorted(listed) == sorted([str(root), *below])
    assert len(listed) == len(set(listed))
    assert budget.work.consumed == len(listed)
    assert expanded == tuple(
        (pattern, tuple(root.glob(pattern))) for pattern in LIVE_SHAPED
    )


def _chain(root: Path, depth: int) -> Path:
    leaf = root / "ignored"
    for index in range(depth):
        leaf = leaf / f"d{index:03d}"
    (leaf / "target").mkdir(parents=True)
    return leaf / "target"


def test_exhausted_work_share_stops_listing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "checkout"
    _chain(root, 30)
    budget = cli._CachePlanBudget(
        cli._AuditWorkBudget(5), 3_600.0, 3_600.0
    )
    with _counted_listings(monkeypatch) as listed:
        with pytest.raises(cli._CachePlanExhausted) as refused:
            cli._expand_cache_globs(root, ("ignored/**/target",), budget)
    assert str(refused.value) == (
        "cache planning exhausted this subject's share of 5 directory listings; "
        "no cache total can be published within this allowance"
    )
    assert len(listed) == 5
    assert budget.work.consumed == 5


def test_walk_that_outlasts_its_wall_share_stops_listing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "checkout"
    _chain(root, 30)
    ticks = iter(range(1_000))
    monkeypatch.setattr(
        cli, "time", types.SimpleNamespace(monotonic=lambda: float(next(ticks)))
    )
    budget = cli._CachePlanBudget(cli._AuditWorkBudget(1_000), 4.0, 8.0, spent=1.0)
    with _counted_listings(monkeypatch) as listed:
        with pytest.raises(cli._CachePlanExhausted) as refused:
            cli._expand_cache_globs(root, ("ignored/**/target",), budget)
    assert str(refused.value) == (
        "cache planning exhausted this subject's share of the fixed wall allowance "
        "of 8 seconds; no cache total can be published within this allowance"
    )
    # The walk starts at tick 0 and reads one tick per listing: the checks at
    # ticks 1 and 2 leave 1 + 1 and 1 + 2 of 4 seconds spent, tick 3 refuses.
    assert len(listed) == 2
    assert budget.spent == 1.0 + 4.0


def test_expired_wall_share_refuses_before_any_listing_or_git(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "checkout"
    _chain(root, 3)
    budget = cli._CachePlanBudget(cli._AuditWorkBudget(1_000), 2.5, 2.5, spent=2.5)

    class NoGit(cli._GitVcs):
        def repository_root(self, repository: Path) -> Path:
            raise AssertionError("Git ran after the planning allowance expired")

    expired = (
        "cache planning exhausted this subject's share of the fixed wall allowance "
        "of 2.5 seconds; no cache total can be published within this allowance"
    )
    with _counted_listings(monkeypatch) as listed:
        with pytest.raises(cli._CachePlanExhausted) as refused:
            cli._expand_cache_globs(root, ("ignored/**/target",), budget)
        assert str(refused.value) == expired
        with pytest.raises(cli._CachePlanExhausted) as refused:
            cli._cache_directories_for_path(
                _config(tmp_path), root, "checkout", NoGit(),
                ("ignored/**/target",), budget=budget,
            )
        assert str(refused.value) == expired
    assert listed == []
    assert budget.work.consumed == 0


def _git_checkout(path: Path) -> None:
    path.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", str(path)], check=True)
    subprocess.run(
        [
            "git", "-C", str(path), "-c", "user.name=fixture",
            "-c", "user.email=fixture@example.invalid",
            "commit", "-q", "--allow-empty", "-m", "fixture",
        ],
        check=True,
    )


def _planning_audit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[cli.Config, Path, Path]:
    config = dataclasses.replace(_config(tmp_path), cache_globs=("ignored/**/target",))
    deep = config.worktrees / "a-deep" / "repo"
    cheap = config.worktrees / "b-cheap" / "repo"
    _git_checkout(deep)
    _git_checkout(cheap)
    deep_cache = _chain(deep, 200)
    (deep_cache / "artifact").write_bytes(b"deep cache content")
    cheap_cache = cheap / "ignored" / "x" / "target"
    cheap_cache.mkdir(parents=True)
    (cheap_cache / "artifact").write_bytes(b"cheap cache content")
    # Registry observations are synthetic and read-only; planning, Git
    # observations, cache traversal and report generation remain real.
    monkeypatch.setattr(cli, "_load_config", lambda *_args: config)
    monkeypatch.setattr(cli, "_locked", lambda *_args, **_kwargs: contextlib.nullcontext())
    monkeypatch.setattr(cli, "_refuse_partial_state", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(cli, "_validate_global_state", lambda *_args: ((), ()))
    monkeypatch.setattr(cli, "_audit_validate_batch_seal_evidence", lambda *_args: ((), ()))
    monkeypatch.setattr(cli, "_all_journal_cache_slots", lambda *_args, **_kwargs: ())
    return config, deep_cache, cheap_cache


def _run_audit(config: cli.Config, tmp_path: Path, *extra: str) -> int:
    storage = tmp_path / "storage"
    storage.mkdir(exist_ok=True)
    return cli.main(
        [
            "--project-root", str(config.root), "audit", "--format", "json",
            "--cache-census-state", str(storage / "census.json"), *extra,
        ]
    )


def _allocated(cache: Path) -> int:
    return sum(path.stat().st_blocks * 512 for path in (cache, cache / "artifact"))


def test_expensive_subject_exhausts_only_its_own_planning_share(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config, _deep_cache, cheap_cache = _planning_audit(tmp_path, monkeypatch)

    result = _run_audit(config, tmp_path, "--cache-work-limit", "60")

    output = capsys.readouterr()
    assert result == 0, output.err
    report = json.loads(output.out)
    rows = {row["slot"]: row for row in report["slots"]}
    assert sorted(rows) == ["a-deep", "b-cheap"]
    # The costly subject is planned first and still leaves its peer an equal
    # share: 60 // 2 listings each, of which the cheap peer needs four.
    assert rows["a-deep"]["cache_status"] == "error"
    assert rows["a-deep"]["cache_bytes"] is None
    exhausted = (
        "cache planning exhausted this subject's share of 30 directory listings; "
        "no cache total can be published within this allowance"
    )
    assert rows["a-deep"]["cache_error"] == exhausted
    # These directories have no registry row, which is always BLOCKED, so the
    # verdict here says nothing about exhaustion. The registered-record test
    # below pins that exhaustion itself becomes a blocking reason.
    assert rows["a-deep"]["verdict"] == "BLOCKED"
    assert rows["b-cheap"]["cache_status"] == "complete"
    assert rows["b-cheap"]["cache_error"] is None
    assert rows["b-cheap"]["cache_bytes"] == _allocated(cheap_cache)
    phases = {phase["name"]: phase for phase in report["metrics"]["phases"]}
    assert phases["cache-planning"]["work"] == {
        "directories_listed": 34,
        "exhausted_subjects": 1,
        "subjects": 2,
    }


def test_sufficient_planning_share_measures_every_subject(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config, deep_cache, cheap_cache = _planning_audit(tmp_path, monkeypatch)

    result = _run_audit(config, tmp_path)

    output = capsys.readouterr()
    assert result == 0, output.err
    rows = {row["slot"]: row for row in json.loads(output.out)["slots"]}
    assert rows["a-deep"]["cache_status"] == "complete"
    assert rows["a-deep"]["cache_bytes"] == _allocated(deep_cache)
    assert rows["b-cheap"]["cache_status"] == "complete"
    assert rows["b-cheap"]["cache_bytes"] == _allocated(cheap_cache)


def test_planning_still_refuses_a_nested_symlinked_cache_name(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config, deep_cache, _cheap_cache = _planning_audit(tmp_path, monkeypatch)
    # Inside an already-matched directory, as Path.glob also reaches it.
    nested = deep_cache / "inner" / "target"
    nested.parent.mkdir()
    nested.symlink_to(tmp_path, target_is_directory=True)

    result = _run_audit(config, tmp_path)

    output = capsys.readouterr()
    assert result == 0, output.err
    rows = {row["slot"]: row for row in json.loads(output.out)["slots"]}
    assert rows["a-deep"]["cache_status"] == "error"
    assert rows["a-deep"]["cache_error"] == f"cache path crosses a symlink: {nested}"
    assert rows["b-cheap"]["cache_status"] == "complete"


def test_planning_shares_divide_what_earlier_subjects_left(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config, _deep_cache, _cheap_cache = _planning_audit(tmp_path, monkeypatch)
    shares: list[tuple[str, float, float, int]] = []
    real = cli._cache_slot_directories

    def spy(
        config: cli.Config,
        cache_slot: cli.CacheSlot,
        *,
        vcs: cli._GitVcs | None = None,
        budget: cli._CachePlanBudget | None = None,
    ) -> tuple[cli.CacheDirectory, ...]:
        assert budget is not None
        shares.append((cache_slot.slot, budget.seconds, budget.spent, budget.work.limit))
        planned = real(config, cache_slot, vcs=vcs, budget=budget)
        shares.append((cache_slot.slot, budget.seconds, budget.spent, budget.work.consumed))
        return planned

    monkeypatch.setattr(cli, "_cache_slot_directories", spy)
    result = _run_audit(
        config, tmp_path, "--cache-work-limit", "1000", "--cache-wall-seconds", "1000"
    )

    assert result == 0, capsys.readouterr().err
    (deep, deep_seconds, deep_before, deep_limit) = shares[0]
    (_, _, deep_spent, deep_used) = shares[1]
    (cheap, cheap_seconds, cheap_before, cheap_limit) = shares[2]
    assert (deep, cheap) == ("a-deep", "b-cheap")
    # Half of each allowance for the first of two subjects; everything that is
    # left for the last. Only walking is charged, and it was charged.
    assert (deep_seconds, deep_limit) == (500.0, 500)
    assert (cheap_seconds, cheap_limit) == (1000.0 - deep_spent, 1000 - deep_used)
    assert deep_before == cheap_before == 0.0
    assert 0.0 < deep_spent < 500.0
    assert deep_used == 203


def test_registered_record_planning_uses_its_share(tmp_path: Path) -> None:
    from wrkslots.tests.test_lifecycle import checkout, command, create, make_project

    project, _repository, _remote = make_project(
        tmp_path, cache_globs=("ignored/**/target",)
    )
    assert create(project).returncode == 0
    cache = _chain(checkout(project), 40)
    (cache / "artifact").write_bytes(b"registered cache content")

    bounded = command(project, "audit", "--format", "json", "--cache-work-limit", "10")
    complete = command(project, "audit", "--format", "json")

    assert bounded.returncode == 0, bounded.stderr
    row = {row["slot"]: row for row in json.loads(bounded.stdout)["slots"]}["slot01"]
    assert row["cache_status"] == "error"
    exhausted = (
        "cache planning exhausted this subject's share of 10 directory listings; "
        "no cache total can be published within this allowance"
    )
    assert row["cache_error"] == exhausted
    assert row["verdict"] == "BLOCKED"
    assert f"cache inspection failed: {exhausted}" in row["reasons"]
    assert complete.returncode == 0, complete.stderr
    row = {row["slot"]: row for row in json.loads(complete.stdout)["slots"]}["slot01"]
    assert row["cache_status"] == "complete"
    assert row["cache_bytes"] == _allocated(cache)
    assert not [r for r in row["reasons"] if r.startswith("cache inspection")]
