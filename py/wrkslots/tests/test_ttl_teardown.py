"""Deterministic TTL teardown against fixture repositories (#1293 slice 4).

Teardown always removes: a pushed rescue, and a failed rescue held by the
verified bundle, both end with the checkout gone and the receipt kept.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_census_cost import _config
from wrkslots.tests.test_ttl_rescue import _git, _remote_ref, _repo


def test_teardown_after_successful_rescue_removes_checkout(tmp_path: Path) -> None:
    repo, remote = _repo(tmp_path)
    _git(repo, "checkout", "-q", "-b", "feat/y")
    (repo / "work.txt").write_text("stranded\n", encoding="utf-8")

    receipt = cli._execute_ttl_teardown(
        repo, slot="slot-c", date_str="20261114", remote="origin",
        bundle_path=tmp_path / "bundles" / "slot-c.bundle",
    )

    assert receipt["removed"] is True
    assert not repo.exists()
    entry = next(
        item for item in receipt["rescue"]["rescued"] if item["branch"] == "feat/y"
    )
    assert entry["pushed"] is True
    assert _remote_ref(remote, entry["rescue_ref"]) == entry["wip_commit"]


def test_failed_rescue_still_tears_down_after_the_bundle(tmp_path: Path) -> None:
    repo, _remote = _repo(tmp_path)
    head = _git(repo, "rev-parse", "HEAD")

    receipt = cli._execute_ttl_teardown(
        repo, slot="slot-d", date_str="20261114", remote="missing-remote",
        bundle_path=tmp_path / "bundles" / "slot-d.bundle",
    )

    assert receipt["removed"] is True
    assert not repo.exists()
    bundle = Path(str(receipt["rescue"]["bundle"]))
    assert bundle.is_file()
    import subprocess

    listed = subprocess.run(
        ["git", "bundle", "list-heads", str(bundle)],
        check=True, capture_output=True, text=True,
    ).stdout
    assert head in listed


def _slot_record(config: cli.Config, repo: Path, remote: Path) -> cli.ActiveRecord:
    coordinator = cli.ProcessIdentity(1, 1, "boot", "host", "/fixture")
    if repo.exists():
        _git(repo, "remote", "add", "origin", str(remote))
    return cli.ActiveRecord(
        slot="slot-f",
        agent="agent-slot-f",
        task="task-slot-f",
        purpose="ttl slot wiring fixture",
        slot_type="agent",
        machine="testhost",
        generation=1,
        created_at="2026-11-01T00:00:00+00:00",
        heartbeat_at="2026-11-10T00:00:00+00:00",
        heartbeat_ttl_seconds=3600,
        owner=None,
        coordinator_lease=coordinator,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(
            cli.Checkout(
                name="repo",
                path=repo.relative_to(config.root).as_posix(),
                repository="repo",
                branch="feat/z",
                start_point="refs/remotes/origin/main",
                remote="origin",
                remote_url_sha256="0" * 64,
                landed_ref="refs/remotes/origin/main",
                head="1" * 40,
            ),
        ),
    )


def test_slot_teardown_rescues_then_removes_at_the_teardown_rung(tmp_path: Path) -> None:
    import datetime as dt
    import subprocess

    config = _config(tmp_path)
    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "-q", "--bare", str(remote)], check=True)
    repo = config.worktrees / "slot-f" / "repo"
    repo.mkdir(parents=True)
    _git(repo, "init", "-q")
    _git(repo, "-c", "user.name=fixture", "-c", "user.email=f@x.invalid",
         "commit", "-q", "--allow-empty", "-m", "base")
    _git(repo, "checkout", "-q", "-b", "feat/z")
    (repo / "work.txt").write_text("stranded\n", encoding="utf-8")
    record = _slot_record(config, repo, remote)
    now = dt.datetime(2026, 11, 14, 12, 0, tzinfo=dt.timezone.utc)

    receipt = cli._execute_ttl_slot_teardown(
        config, record, now=now, thresholds=cli._ttl_thresholds(0, 86_400, 172_800, 259_200)
    )

    assert receipt["removed"] is True
    assert not (config.worktrees / "slot-f").exists()
    assert receipt["rescue"]["stage"] == "teardown"
    entries = {
        entry["branch"]: entry
        for checkout in receipt["rescue"]["checkouts"]
        for entry in checkout["rescued"]
    }
    assert entries["feat/z"]["pushed"] is True
    assert _remote_ref(remote, entries["feat/z"]["rescue_ref"])


def test_slot_teardown_refuses_before_the_teardown_rung(tmp_path: Path) -> None:
    import datetime as dt

    config = _config(tmp_path)
    record = _slot_record(config, config.worktrees / "slot-f" / "repo", tmp_path / "r.git")
    now = dt.datetime(2026, 11, 11, 12, 0, tzinfo=dt.timezone.utc)  # warn rung
    with pytest.raises(cli.Refusal):
        cli._execute_ttl_slot_teardown(
            config, record, now=now,
            thresholds=cli._ttl_thresholds(0, 86_400, 172_800, 259_200),
        )


def test_teardown_refuses_a_path_that_is_no_checkout(tmp_path: Path) -> None:
    plain = tmp_path / "plain"
    plain.mkdir()
    with pytest.raises(cli.Refusal):
        cli._execute_ttl_teardown(
            plain, slot="slot-e", date_str="20261114", remote="origin",
            bundle_path=tmp_path / "bundles" / "slot-e.bundle",
        )
    assert plain.exists()
