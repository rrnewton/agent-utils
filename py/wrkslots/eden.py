"""Eden checkout support for wrkslots (#1204 wrkslots-eden-checkouts).

An Eden checkout is a second checkout kind beside git worktrees. This
module holds the pure, fake-friendly logic: host-wide cap accounting,
redirection and usage parsing, Sapling salvage commands, and the
remove verification. Process execution is injected as a runner so
unit tests never touch a real daemon; ``cli.py`` supplies the real
runner. Nothing here creates or removes a checkout by itself.
"""

from __future__ import annotations

import json
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path

#: Per-host cap on Eden checkouts, set by the owner for this host.
HOST_CAP = 3

#: Fixed markers stored in the git-shaped fields of an Eden row. The
#: live Eden facts (state dir, revision, targets) are always queried
#: from the daemon, never trusted from these markers.
EDEN_BRANCH = "eden"
EDEN_REMOTE = "eden"
EDEN_LANDED_REF = "refs/remotes/eden/main"

#: Runner: argv -> (return code, stdout, stderr).
Runner = Callable[[Sequence[str]], tuple[int, str, str]]


class EdenError(RuntimeError):
    """An Eden operation could not be completed safely."""


@dataclass(frozen=True)
class Redirection:
    """One redirected path of an Eden checkout."""

    repo_path: str
    kind: str
    target: str


@dataclass(frozen=True)
class CheckoutInfo:
    """Facts ``eden info`` reports for one mount."""

    mount: str
    state_dir: str
    backing_repo: str
    revision: str


def config_dirs_from_cmdlines(cmdlines: Sequence[str]) -> tuple[str, ...]:
    """Eden state dirs named by running daemons' ``--edenDir`` flags."""

    found: list[str] = []
    for line in cmdlines:
        parts = line.split()
        for index, part in enumerate(parts):
            if part == "--edenDir" and index + 1 < len(parts):
                value = parts[index + 1]
                if value not in found:
                    found.append(value)
            elif part.startswith("--edenDir="):
                value = part.partition("=")[2]
                if value and value not in found:
                    found.append(value)
    return tuple(sorted(found))


def discover_config_dirs(
    cmdlines: Sequence[str], known: Sequence[str]
) -> tuple[str, ...]:
    """Every Eden state dir to count: daemon-reported plus known ones."""

    merged = list(config_dirs_from_cmdlines(cmdlines))
    for value in known:
        if value not in merged:
            merged.append(value)
    return tuple(sorted(merged))


def parse_mount_list(stdout: str) -> tuple[str, ...]:
    """Mount paths from ``eden list`` output, one per line."""

    return tuple(
        line.strip() for line in stdout.splitlines() if line.strip()
    )


def count_checkouts(runner: Runner, config_dirs: Sequence[str]) -> tuple[str, ...]:
    """All Eden mounts on the host, deduplicated by mount path.

    Fail-closed: a state dir whose daemon cannot be queried raises,
    so an uncertain count never reads as room under the cap.
    """

    mounts: list[str] = []
    for config_dir in config_dirs:
        rc, stdout, stderr = runner(
            ["eden", "--config-dir", config_dir, "list"]
        )
        if rc != 0:
            raise EdenError(
                f"cannot list Eden checkouts in {config_dir}: "
                f"{stderr.strip() or stdout.strip() or 'eden list failed'}"
            )
        for mount in parse_mount_list(stdout):
            if mount not in mounts:
                mounts.append(mount)
    return tuple(mounts)


def assert_under_cap(mounts: Sequence[str], cap: int = HOST_CAP) -> None:
    """Refuse when the host already holds ``cap`` Eden checkouts."""

    if len(mounts) >= cap:
        raise EdenError(
            f"host Eden cap reached: {len(mounts)} checkout(s) at cap {cap}: "
            + ", ".join(mounts)
        )


def parse_info(stdout: str) -> CheckoutInfo:
    """Parse ``eden info <mount>`` JSON."""

    try:
        raw = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise EdenError(f"eden info returned invalid JSON: {exc}") from exc
    if not isinstance(raw, dict):
        raise EdenError("eden info returned a non-object JSON value")
    fields: dict[str, str] = {}
    for key in ("mount", "state_dir", "backing_repo", "checked_out_revision"):
        value = raw.get(key)
        if not isinstance(value, str) or not value:
            raise EdenError(f"eden info is missing {key}")
        fields[key] = value
    return CheckoutInfo(
        mount=fields["mount"],
        state_dir=fields["state_dir"],
        backing_repo=fields["backing_repo"],
        revision=fields["checked_out_revision"],
    )


def parse_redirections(stdout: str) -> tuple[Redirection, ...]:
    """Parse ``eden redirect list --json`` output."""

    try:
        raw = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise EdenError(f"eden redirect list returned invalid JSON: {exc}") from exc
    if not isinstance(raw, list):
        raise EdenError("eden redirect list returned a non-list JSON value")
    result: list[Redirection] = []
    for item in raw:
        if not isinstance(item, dict):
            raise EdenError("eden redirect list entry is not an object")
        repo_path = item.get("repo_path")
        kind = item.get("type")
        target = item.get("target")
        if not all(isinstance(v, str) and v for v in (repo_path, kind, target)):
            raise EdenError("eden redirect list entry lacks repo_path/type/target")
        result.append(
            Redirection(repo_path=str(repo_path), kind=str(kind), target=str(target))
        )
    return tuple(result)


def parse_orphaned_bytes(stdout: str) -> int:
    """Orphaned-redirection bytes from ``eden du --json`` output."""

    try:
        raw = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise EdenError(f"eden du returned invalid JSON: {exc}") from exc
    if not isinstance(raw, dict):
        raise EdenError("eden du returned a non-object JSON value")
    value = raw.get("orphaned_redirections")
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise EdenError("eden du lacks a valid orphaned_redirections count")
    return value


def salvage_needed(sl_status: str, current: str, recorded: str) -> bool:
    """Whether Sapling work must be preserved before removal."""

    return bool(sl_status.strip()) or current != recorded


def salvage_commands(mount: str, dest: Path) -> tuple[tuple[str, ...], ...]:
    """Commands preserving uncommitted work as a patch plus a bundle.

    The bundle holds the draft stack; the patch holds the working-copy
    diff. Both land in the salvage archive and are read back by the
    caller before removal proceeds, exactly like git bundles.
    """

    patch = str(dest / "working-copy.patch")
    bundle = str(dest / "draft-stack.bundle")
    return (
        ("sl", "-R", mount, "diff", "--git", "-o", patch),
        ("sl", "-R", mount, "export", "--git", "-o", patch + ".stack", "."),
        ("sl", "-R", mount, "bundle", "--all", bundle),
    )


def verify_removed(
    mounts_after: Sequence[str],
    mount: str,
    targets: Sequence[str],
    existing_paths: Sequence[str],
    orphaned_before: int,
    orphaned_after: int,
) -> tuple[str, ...]:
    """Orphan checks after ``eden remove``; empty means fully removed.

    ``eden remove`` leaves buck-out targets behind, so removal is
    only proven when the mount is unlisted, every recorded target
    path is gone, and orphaned-redirection bytes did not grow.
    """

    problems: list[str] = []
    if mount in mounts_after:
        problems.append(f"mount still listed by eden: {mount}")
    for target in targets:
        if target in existing_paths:
            problems.append(f"redirection target still present: {target}")
    if orphaned_after > orphaned_before:
        problems.append(
            f"orphaned redirections grew: {orphaned_before} -> {orphaned_after} bytes"
        )
    return tuple(problems)
