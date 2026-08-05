"""The one real :class:`~pr_landing_planner.host.VcsHost`: GitHub via ``gh`` + a local ``git`` clone.

PR listing (and the rollup + labels carried on each :class:`~pr_landing_planner.model.RawPr`) comes
from a single ``gh pr list --json`` call; the conflict / ancestry / freshness operations are plain
``git`` against a local clone. An optional ``--net-wrapper`` prefixes each ``gh`` and ``git fetch``
command, while ``--gh-cmd`` supports installations that use an authenticated wrapper. Gate-check
names and flaky signatures live in classifier configuration, not this host.
"""

from __future__ import annotations

import json
import shlex
import subprocess
import sys
from collections.abc import Mapping, Sequence
from concurrent.futures import ThreadPoolExecutor

from pr_landing_planner.classify import parse_rollup
from pr_landing_planner.model import CheckRun, RawPr

GH_FIELDS: tuple[str, ...] = (
    "number",
    "title",
    "author",
    "baseRefName",
    "headRefName",
    "headRefOid",
    "isDraft",
    "mergeable",
    "reviewDecision",
    "createdAt",
    "updatedAt",
    "additions",
    "deletions",
    "labels",
    "statusCheckRollup",
)

#: The heavy ``statusCheckRollup`` field makes a single ``gh pr list`` over a large open set 504 at
#: the GraphQL layer (measured: fails at 60 PRs on rrnewton/hermit). So the light metadata is fetched
#: in one cheap list call (:data:`LIGHT_FIELDS`) and the rollup is enriched per PR, in parallel,
#: below. Each per-PR ``gh pr view`` is small and reliable; a rollup that still fails degrades that one
#: PR to "no checks" (classified pending) with a LOUD stderr NOTE rather than aborting the whole plan.
LIGHT_FIELDS: tuple[str, ...] = tuple(f for f in GH_FIELDS if f != "statusCheckRollup")

#: Concurrency for the per-PR rollup enrichment. Bounded so we never fan out hundreds of ``gh``
#: processes; the work is network-bound so a small pool already hides most latency.
_ROLLUP_WORKERS = 8


class HostCommandError(RuntimeError):
    """A ``gh`` / ``git`` command failed; carries the command + captured stderr for debugging."""

    def __init__(self, cmd: Sequence[str], returncode: int, stderr: str) -> None:
        super().__init__(f"command failed ({returncode}): {shlex.join(cmd)}\n{stderr.strip()}")
        self.returncode = returncode


def _run(
    cmd: Sequence[str], cwd: str | None, allowed: tuple[int, ...] = (0,)
) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(list(cmd), cwd=cwd, capture_output=True, text=True)
    if proc.returncode not in allowed:
        raise HostCommandError(cmd, proc.returncode, proc.stderr)
    return proc


def _str(m: Mapping[str, object], key: str) -> str:
    val = m.get(key)
    if isinstance(val, str):
        return val
    if isinstance(val, (int, float, bool)):
        return str(val)
    return ""


def _int(m: Mapping[str, object], key: str) -> int:
    val = m.get(key)
    return val if isinstance(val, int) and not isinstance(val, bool) else 0


def _bool(m: Mapping[str, object], key: str) -> bool:
    return bool(m.get(key) is True)


def _author_login(value: object) -> str:
    if isinstance(value, dict):
        login = value.get("login")
        if isinstance(login, str):
            return login
    return "unknown"


def _labels(value: object) -> tuple[str, ...]:
    if not isinstance(value, list):
        return ()
    out: list[str] = []
    for entry in value:
        if isinstance(entry, dict):
            name = entry.get("name")
            if isinstance(name, str):
                out.append(name)
    return tuple(out)


class GitHubHost:
    """Talk to GitHub through ``gh`` and to a local clone through ``git``."""

    def __init__(
        self,
        *,
        git_dir: str = ".",
        remote: str = "origin",
        net_wrapper: Sequence[str] = (),
        gh_cmd: str = "gh",
    ) -> None:
        self._git_dir = git_dir
        self._remote = remote
        self._wrapper = tuple(net_wrapper)
        self._gh = gh_cmd

    def _net(self, cmd: Sequence[str]) -> list[str]:
        return [*self._wrapper, *cmd]

    def list_open_prs(self, repo: str, base: str | None) -> tuple[RawPr, ...]:
        """List open pull requests and attach each request's latest check rollup."""

        # One cheap list for the light metadata (no rollup -> no GraphQL 504 on a large open set)...
        proc = _run(
            self._net(
                [
                    self._gh, "pr", "list",
                    "--repo", repo,
                    "--state", "open",
                    "--limit", "500",
                    "--json", ",".join(LIGHT_FIELDS),
                ]
            ),
            cwd=None,
        )
        raw: object = json.loads(proc.stdout) if proc.stdout.strip() else []
        if not isinstance(raw, list):
            raise HostCommandError(["gh", "pr", "list"], 0, "expected a JSON array from gh")
        entries: list[dict[str, object]] = [
            {str(k): v for k, v in entry.items()} for entry in raw if isinstance(entry, dict)
        ]
        # ...then enrich each PR's rollup with a small per-PR ``gh pr view``, in parallel.
        numbers = [_int(obj, "number") for obj in entries]
        rollups = self._fetch_rollups(repo, numbers)
        prs: list[RawPr] = []
        for obj in entries:
            number = _int(obj, "number")
            prs.append(
                RawPr(
                    number=number,
                    head_ref=_str(obj, "headRefName"),
                    base_ref=_str(obj, "baseRefName"),
                    api_head_sha=_str(obj, "headRefOid"),
                    title=_str(obj, "title"),
                    author=_author_login(obj.get("author")),
                    is_draft=_bool(obj, "isDraft"),
                    mergeable=_str(obj, "mergeable"),
                    review_decision=_str(obj, "reviewDecision"),
                    created_at=_str(obj, "createdAt"),
                    updated_at=_str(obj, "updatedAt"),
                    additions=_int(obj, "additions"),
                    deletions=_int(obj, "deletions"),
                    labels=_labels(obj.get("labels")),
                    checks=rollups.get(number, ()),
                )
            )
        return tuple(prs)

    def _fetch_rollups(
        self, repo: str, numbers: Sequence[int]
    ) -> Mapping[int, tuple[CheckRun, ...]]:
        """Fetch each PR's ``statusCheckRollup`` with a small per-PR ``gh pr view``, in parallel.

        A single per-PR failure degrades that PR to no checks (classified pending) with a LOUD stderr
        NOTE — No Silent Failure — instead of aborting the whole plan. The returned mapping is by PR
        number, so the caller's order is unaffected by completion order (result stays deterministic).
        """
        rollups: dict[int, tuple[CheckRun, ...]] = {}
        failed: list[int] = []

        def one(number: int) -> tuple[int, tuple[CheckRun, ...] | None]:
            try:
                proc = _run(
                    self._net(
                        [
                            self._gh, "pr", "view", str(number),
                            "--repo", repo,
                            "--json", "number,headRefOid,statusCheckRollup",
                        ]
                    ),
                    cwd=None,
                )
            except HostCommandError:
                return number, None
            obj = json.loads(proc.stdout) if proc.stdout.strip() else {}
            if not isinstance(obj, dict):
                return number, None
            return number, parse_rollup(
                obj.get("statusCheckRollup"), head_sha=_str(obj, "headRefOid")
            )

        if numbers:
            with ThreadPoolExecutor(max_workers=_ROLLUP_WORKERS) as pool:
                for number, checks in pool.map(one, numbers):
                    if checks is None:
                        failed.append(number)
                    else:
                        rollups[number] = checks
        if failed:
            listed = ",".join(f"#{n}" for n in sorted(failed))
            print(
                f"pr-landing-planner: NOTE: rollup fetch failed for {len(failed)} PR(s) "
                f"({listed}); treating them as pending (no checks)",
                file=sys.stderr,
            )
        return rollups

    def prefetch_refs(self, refspecs: Sequence[tuple[str, str]]) -> dict[str, str]:
        """Fetch all requested refs in one operation and return their object IDs by destination."""

        # ONE `git fetch` for every (source, dest) — a single remote round-trip instead of a per-PR
        # fan-out. Measured cost of the two shapes (2026-08-04, warm, 25 hermit PR heads into a local
        # rrnewton/hermit clone): N separate `git fetch` = 21.5 s wall / 14.3 s sys (≈0.86 s/PR, almost
        # all process-spawn + round-trip overhead); one batched `git fetch` = 0.85 s wall / 0.57 s sys
        # — ~25× faster and O(1) in round-trips. This is why the planner's default conflict detector is
        # `merge-tree`: once the graph is local, each merge-tree probe is ~37 ms, so conflict-analysing
        # the whole open set costs seconds, not the "expensive fan-out" the per-PR model implied.
        if not refspecs:
            return {}
        pairs = [f"+{source}:{dest}" for source, dest in refspecs]
        _run(
            self._net(["git", "fetch", "--quiet", "--no-tags", self._remote, *pairs]),
            cwd=self._git_dir,
        )
        resolved: dict[str, str] = {}
        for _source, dest in refspecs:
            resolved[dest] = _run(["git", "rev-parse", dest], cwd=self._git_dir).stdout.strip()
        return resolved

    def merge_tree(self, left: str, right: str) -> tuple[str, ...]:
        """Return paths that conflict when merging the two object IDs."""

        proc = _run(
            ["git", "merge-tree", "--write-tree", "--name-only", "--messages", left, right],
            cwd=self._git_dir,
            allowed=(0, 1),
        )
        if proc.returncode == 0:
            return ()
        lines = proc.stdout.splitlines()
        paths: list[str] = []
        for line in lines[1:]:  # first line is the tree oid
            candidate = line.strip()
            if not candidate:
                break
            paths.append(candidate)
        return tuple(sorted(set(paths)))

    def is_ancestor(self, ancestor: str, descendant: str) -> bool:
        """Return whether *ancestor* is reachable from *descendant*."""

        proc = _run(
            ["git", "merge-base", "--is-ancestor", ancestor, descendant],
            cwd=self._git_dir,
            allowed=(0, 1),
        )
        return proc.returncode == 0

    def changed_files(self, base_sha: str, head_sha: str) -> frozenset[str]:
        """Return paths changed between the base and head object IDs."""

        merge_base = _run(
            ["git", "merge-base", base_sha, head_sha], cwd=self._git_dir
        ).stdout.strip()
        out = _run(
            ["git", "diff", "--name-only", f"{merge_base}...{head_sha}"], cwd=self._git_dir
        ).stdout
        return frozenset(line for line in out.splitlines() if line)

    def commits_behind(self, head_sha: str, base_sha: str) -> int:
        """Return how many commits *head_sha* is behind *base_sha*."""

        out = _run(
            ["git", "rev-list", "--count", f"{head_sha}..{base_sha}"], cwd=self._git_dir
        ).stdout.strip()
        try:
            return int(out)
        except ValueError:
            return 0


__all__ = ["GitHubHost", "HostCommandError", "GH_FIELDS", "LIGHT_FIELDS"]
