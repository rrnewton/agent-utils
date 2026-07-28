"""The one real :class:`~pr_landing_planner.host.VcsHost`: GitHub via ``gh`` + a local ``git`` clone.

PR listing (and the rollup + labels carried on each :class:`~pr_landing_planner.model.RawPr`) comes
from a single ``gh pr list --json`` call; the conflict / ancestry / freshness operations are plain
``git`` against a local clone. A pluggable network wrapper (``--net-wrapper with-proxy``) prefixes
every ``gh`` / ``git fetch`` so it runs on a proxied host or bare, and ``--gh-cmd`` swaps in a
wrapper such as ``./scripts/gh_human``. Nothing here is DeepScry-specific: the gate-check name and
flaky signatures live in the classifier config, not this host.
"""

from __future__ import annotations

import json
import shlex
import subprocess
from collections.abc import Mapping, Sequence

from pr_landing_planner.classify import parse_rollup
from pr_landing_planner.model import RawPr

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
        proc = _run(
            self._net(
                [
                    self._gh, "pr", "list",
                    "--repo", repo,
                    "--state", "open",
                    "--limit", "500",
                    "--json", ",".join(GH_FIELDS),
                ]
            ),
            cwd=None,
        )
        raw: object = json.loads(proc.stdout) if proc.stdout.strip() else []
        if not isinstance(raw, list):
            raise HostCommandError(["gh", "pr", "list"], 0, "expected a JSON array from gh")
        prs: list[RawPr] = []
        for entry in raw:
            if not isinstance(entry, dict):
                continue
            obj: dict[str, object] = {str(k): v for k, v in entry.items()}
            prs.append(
                RawPr(
                    number=_int(obj, "number"),
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
                    checks=parse_rollup(obj.get("statusCheckRollup")),
                )
            )
        return tuple(prs)

    def fetch_ref(self, source: str, dest: str) -> str:
        _run(
            self._net(
                ["git", "fetch", "--quiet", "--no-tags", self._remote, f"+{source}:{dest}"]
            ),
            cwd=self._git_dir,
        )
        return _run(["git", "rev-parse", dest], cwd=self._git_dir).stdout.strip()

    def merge_tree(self, left: str, right: str) -> tuple[str, ...]:
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
        proc = _run(
            ["git", "merge-base", "--is-ancestor", ancestor, descendant],
            cwd=self._git_dir,
            allowed=(0, 1),
        )
        return proc.returncode == 0

    def changed_files(self, base_sha: str, head_sha: str) -> frozenset[str]:
        merge_base = _run(
            ["git", "merge-base", base_sha, head_sha], cwd=self._git_dir
        ).stdout.strip()
        out = _run(
            ["git", "diff", "--name-only", f"{merge_base}...{head_sha}"], cwd=self._git_dir
        ).stdout
        return frozenset(line for line in out.splitlines() if line)

    def commits_behind(self, head_sha: str, base_sha: str) -> int:
        out = _run(
            ["git", "rev-list", "--count", f"{head_sha}..{base_sha}"], cwd=self._git_dir
        ).stdout.strip()
        try:
            return int(out)
        except ValueError:
            return 0


__all__ = ["GitHubHost", "HostCommandError", "GH_FIELDS"]
