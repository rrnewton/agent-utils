"""Tests for GitHubHost.list_open_prs's two-phase collection: light list + parallel rollup enrich.

The heavy ``statusCheckRollup`` field makes a single ``gh pr list`` 504 on a large open set (measured
at 60 PRs). GitHubHost therefore fetches the light metadata in one list call and enriches each PR's
rollup with a small per-PR ``gh pr view``, in parallel, degrading a single failed rollup to "no
checks" rather than aborting the whole plan. These tests pin both behaviours by faking the runner.
"""

from __future__ import annotations

import json
import subprocess
from collections.abc import Sequence

import pytest

from pr_landing_planner import githubhost
from pr_landing_planner.githubhost import GitHubHost, HostCommandError


def _completed(stdout: str) -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(args=[], returncode=0, stdout=stdout, stderr="")


def _fake_run_factory(*, fail_view_for: frozenset[int] = frozenset()):
    """Build a fake ``_run`` that answers the light list and each per-PR ``gh pr view``."""

    def fake_run(
        cmd: Sequence[str], cwd: str | None, allowed: tuple[int, ...] = (0,)
    ) -> subprocess.CompletedProcess[str]:
        parts = list(cmd)
        if "list" in parts:
            assert "statusCheckRollup" not in ",".join(parts), "light list must omit rollup"
            return _completed(
                json.dumps(
                    [
                        {"number": 1, "headRefName": "a", "baseRefName": "main"},
                        {"number": 2, "headRefName": "b", "baseRefName": "main"},
                    ]
                )
            )
        if "view" in parts:
            number = int(parts[parts.index("view") + 1])
            if number in fail_view_for:
                raise HostCommandError(cmd, 1, "boom")
            rollup = [{"__typename": "CheckRun", "name": "ci", "status": "COMPLETED",
                       "conclusion": "SUCCESS"}]
            return _completed(json.dumps({"number": number, "statusCheckRollup": rollup}))
        raise AssertionError(f"unexpected command: {parts}")

    return fake_run


def test_list_open_prs_enriches_rollup_per_pr(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(githubhost, "_run", _fake_run_factory())
    prs = GitHubHost().list_open_prs("owner/repo", "main")
    assert [p.number for p in prs] == [1, 2]  # order follows the light list, not completion order
    assert all(len(p.checks) == 1 for p in prs)  # every PR got its rollup


def test_list_open_prs_degrades_failed_rollup(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setattr(githubhost, "_run", _fake_run_factory(fail_view_for=frozenset({2})))
    prs = GitHubHost().list_open_prs("owner/repo", "main")
    by = {p.number: p for p in prs}
    assert len(by[1].checks) == 1  # healthy PR still enriched
    assert by[2].checks == ()  # failed rollup degrades to no checks, PR is NOT dropped
    err = capsys.readouterr().err
    assert "#2" in err and "rollup fetch failed" in err  # LOUD note, not silent
