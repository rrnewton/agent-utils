"""Deterministic TTL teardown against fixture repositories (#1293 slice 4).

Teardown always removes: a pushed rescue, and a failed rescue held by the
verified bundle, both end with the checkout gone and the receipt kept.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from wrkslots import cli
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
    (entry,) = receipt["rescue"]["rescued"]
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


def test_teardown_refuses_a_path_that_is_no_checkout(tmp_path: Path) -> None:
    plain = tmp_path / "plain"
    plain.mkdir()
    with pytest.raises(cli.Refusal):
        cli._execute_ttl_teardown(
            plain, slot="slot-e", date_str="20261114", remote="origin",
            bundle_path=tmp_path / "bundles" / "slot-e.bundle",
        )
    assert plain.exists()
