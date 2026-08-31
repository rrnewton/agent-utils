#!/usr/bin/env python3
"""Safely manage one active agent per Git worktree slot."""

from __future__ import annotations

import argparse
import configparser
import contextlib
import dataclasses
import datetime as dt
import errno
import fcntl
import fnmatch
import hashlib
import json
import os
import re
import shlex
import socket
import stat
import subprocess
import sys
import tempfile
import time
import uuid
from collections.abc import Callable, Iterator, Mapping, Sequence, Set as AbstractSet
from pathlib import Path

from wrkslots import __version__

VERSION = __version__
SCHEMA = 2
CONFIG_NAME = ".wrkslots.yml"
# Configuration keys that may be absent. Absent is a meaning, not a default to
# be materialised: no `max_active_slots` key means allocation is uncapped.
OPTIONAL_CONFIG_KEYS = frozenset(
    {
        "max_active_slots",
        "layout",
        "cache_globs",
        "repo_cache_globs",
        "post_provision_hooks",
        "disk_advisory_bytes",
        "disk_provisioning_floor_bytes",
        "disk_emergency_bytes",
    }
)
# Fields `init --repair` may overwrite when the caller names a different value.
# Everything else is refused, because a configuration says where live state
# already is and rewriting that relocates rather than repairs.
_CONFIG_REPAIR_UPDATABLE = frozenset({"schema", "max_active_slots"})
NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
REMOTE_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
GIB = 1024**3
HOLD_SCHEMA = 1
EVENT_SCHEMA = 1
SLOT_TYPES = ("agent", "validate")
HISTORICAL_ACTIVE_STATUSES = frozenset(
    {"active", "lease-quarantined", "owner-lease-revoked", "release-requested"}
)
HISTORICAL_TASK_NOT_RECORDED = "no task recorded in worktree-state.json"
HISTORICAL_PURPOSE_NOT_RECORDED = "no purpose recorded in worktree-state.json"


class Refusal(RuntimeError):
    """An expected fail-closed refusal with an actionable message."""

    def __init__(self, message: str, *, remedy: str | None = None) -> None:
        super().__init__(message)
        self.remedy = remedy


class StateError(Refusal):
    """A corrupt, partial, or incompatible state refusal."""


@dataclasses.dataclass(frozen=True)
class Config:
    """Validated project configuration and resolved local paths."""

    root: Path
    config_path: Path
    worktrees: Path
    control: Path
    machine: str
    default_remote: str
    default_landed_ref: str
    heartbeat_ttl_seconds: int
    liveness_command: Path
    max_active_slots: int | None = None
    layout: str = "nested"
    cache_globs: tuple[str, ...] = ()
    repo_cache_globs: tuple[tuple[str, tuple[str, ...]], ...] = ()
    post_provision_hooks: tuple[str, ...] = ()
    disk_advisory_bytes: int | None = None
    disk_provisioning_floor_bytes: int | None = None
    disk_emergency_bytes: int | None = None


@dataclasses.dataclass(frozen=True)
class ProcessIdentity:
    """One exact operating-system process generation."""

    pid: int
    start_ticks: int
    boot_id: str
    host_id: str
    cgroup_path: str


@dataclasses.dataclass(frozen=True)
class Checkout:
    """Durable Git identity and publication evidence for one checkout."""

    name: str
    path: str
    repository: str
    branch: str
    start_point: str
    remote: str
    remote_url_sha256: str
    landed_ref: str
    head: str
    containing_remote_refs: tuple[str, ...] = ()
    vcs: str = "git"


@dataclasses.dataclass(frozen=True)
class Handoff:
    """Owner-recorded evidence that a slot is ready for later removal."""

    recorded_at: str
    validation: tuple[str, ...]
    limitations: tuple[str, ...]
    continuation: str


@dataclasses.dataclass(frozen=True)
class _ImportSource:
    """Exact historical row retained when an older slot enters this registry."""

    format: str
    path: str
    file_sha256: str
    row_sha256: str
    status: str
    row: dict[str, object]


@dataclasses.dataclass(frozen=True)
class _HistoricalImport:
    """Validated inputs recovered from one older registry row."""

    agent: str
    task: str
    purpose: str
    created_at: str
    owner: ProcessIdentity
    checkouts: tuple[Checkout, ...]
    source: _ImportSource


@dataclasses.dataclass(frozen=True)
class ActiveRecord:
    """One active slot row in a machine state file."""

    slot: str
    agent: str
    task: str
    purpose: str
    slot_type: str
    machine: str
    generation: int
    created_at: str
    heartbeat_at: str
    heartbeat_ttl_seconds: int
    owner: ProcessIdentity | None
    coordinator_lease: ProcessIdentity
    coordinator_recovery_note: str | None
    handoff: Handoff | None
    checkouts: tuple[Checkout, ...]
    layout: str | None = None
    import_source: _ImportSource | None = None


@dataclasses.dataclass(frozen=True)
class ActiveState:
    """The complete active-slot state for one machine."""

    machine: str
    revision: int
    slots: tuple[ActiveRecord, ...]


@dataclasses.dataclass(frozen=True)
class ArchiveState:
    """Completed removal records for one machine."""

    machine: str
    revision: int
    records: tuple[dict[str, object], ...]


@dataclasses.dataclass(frozen=True)
class PlannedCheckout:
    """Validated inputs for one checkout during create."""

    name: str
    destination: str
    repository: str
    branch: str
    start_point: str
    remote: str
    remote_url_sha256: str
    landed_ref: str


@dataclasses.dataclass(frozen=True)
class CacheSlot:
    """One registered or unregistered slot considered for cache cleanup."""

    slot: str
    machine: str
    checkouts: tuple[Checkout, ...]
    state: str = "active"


@dataclasses.dataclass(frozen=True)
class CacheDirectory:
    """A validated regenerable directory within one checkout."""

    path: Path
    checkout_root: Path
    checkout_device: int
    checkout_inode: int
    checkout_mount_id: int


def _utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds")


def _short_hostname() -> str:
    name = socket.gethostname().split(".", 1)[0]
    _validate_name(name, "machine")
    return name


def _validate_name(value: str, label: str) -> str:
    if not NAME_RE.fullmatch(value):
        raise Refusal(
            f"invalid {label} {value!r}; use 1-64 letters, digits, '.', '_', or '-'"
        )
    return value


def _validate_remote(value: str) -> str:
    if (
        not REMOTE_RE.fullmatch(value)
        or value.startswith("-")
        or value.endswith("/")
        or ".." in value
    ):
        raise Refusal(f"invalid remote name {value!r}")
    return value


def _require_coordinator_authorized(args: argparse.Namespace, action: str) -> None:
    if getattr(args, "coordinator_authorized", False):
        return
    raise Refusal(
        f"{action} is coordinator-owned. state: REFUSED -- no worktree or lifecycle "
        "record was changed. remedy: ask the coordinator to perform the operation; a "
        "coordinator-delegated wrapper may rerun it with --coordinator-authorized"
    )


def _require_slot_type(args: argparse.Namespace, action: str) -> str:
    value = getattr(args, "slot_type", None)
    if isinstance(value, str) and value in SLOT_TYPES:
        return value
    raise Refusal(
        f"{action} must record whether this is an agent or validate slot. state: REFUSED "
        "-- no worktree or lifecycle record was changed. remedy: rerun with "
        "--slot-type agent for authored work, or --slot-type validate for a disposable "
        "validation checkout whose evidence lives outside the slot"
    )


def _validate_ref(value: str, label: str) -> str:
    if not value or value.startswith("-") or any(ch.isspace() for ch in value):
        raise Refusal(f"invalid {label} {value!r}")
    if ".." in value or value.endswith("/") or value.endswith("."):
        raise Refusal(f"invalid {label} {value!r}")
    return value


def _validate_full_ref(value: str, label: str) -> str:
    _validate_ref(value, label)
    forbidden = set(" ~^:?*[\\")
    parts = value.split("/")
    if (
        not value.startswith("refs/")
        or len(parts) < 3
        or any(not part or part.startswith(".") or part.endswith(".lock") for part in parts)
        or "@{" in value
        or any(ord(character) < 32 or ord(character) == 127 for character in value)
        or any(character in forbidden for character in value)
    ):
        raise Refusal(f"invalid full {label} {value!r}")
    return value


def _validate_layout(value: str) -> str:
    if value not in {"nested", "flat"}:
        raise Refusal(f"invalid layout {value!r}; expected 'nested' or 'flat'")
    return value


def _control_directory_for(worktrees: Path, layout: str) -> Path:
    if worktrees.name == "slots":
        return worktrees.parent
    return worktrees if layout == "nested" else worktrees.parent


def _validate_cache_glob(value: str) -> str:
    if not value or value != value.strip() or "\x00" in value:
        raise Refusal("cache globs must be non-empty relative paths without surrounding space")
    candidate = Path(value)
    if candidate.is_absolute() or ".." in candidate.parts:
        raise Refusal(f"cache glob must stay inside each checkout: {value!r}")
    normalized = candidate.as_posix()
    if not normalized or normalized == ".":
        raise Refusal("cache glob must not name the checkout root")
    parts = Path(normalized).parts
    if any("**" in part and part != "**" for part in parts):
        raise Refusal(
            "cache glob recursive wildcards ('**') must occupy a complete path component"
        )
    if any(part == ".git" for part in parts):
        raise Refusal("cache globs must never name Git administrative data")
    if any(character in parts[0] for character in "*?["):
        raise Refusal(
            "cache glob's first path component must be literal so it cannot match source broadly"
        )
    return normalized


def _glob_matches_path(pattern: str, path: str) -> bool:
    """Match one slash-delimited path without allowing '*' to cross a slash."""

    pattern_parts = tuple(part for part in pattern.split("/") if part)
    path_parts = tuple(part for part in path.split("/") if part)

    def matches(pattern_index: int, path_index: int) -> bool:
        if pattern_index == len(pattern_parts):
            return path_index == len(path_parts)
        part = pattern_parts[pattern_index]
        if part == "**":
            return matches(pattern_index + 1, path_index) or (
                path_index < len(path_parts) and matches(pattern_index, path_index + 1)
            )
        return (
            path_index < len(path_parts)
            and fnmatch.fnmatchcase(path_parts[path_index], part)
            and matches(pattern_index + 1, path_index + 1)
        )

    return matches(0, 0)


def _cache_glob_contains_path(pattern: str, path: str) -> bool:
    parts = tuple(part for part in path.split("/") if part)
    return any(
        _glob_matches_path(pattern, "/".join(parts[:length]))
        for length in range(1, len(parts) + 1)
    )


def _validate_hook(value: str) -> str:
    if not value.strip() or "\x00" in value:
        raise Refusal("post-provision hooks must be non-empty shell commands")
    return value


def _string_tuple(
    value: object,
    label: str,
    validator: Callable[[str], str],
    *,
    unique: bool = True,
) -> tuple[str, ...]:
    result: list[str] = []
    for index, item in enumerate(_as_list(value, label)):
        raw = _as_str(item, f"{label}[{index}]")
        validated = validator(raw)
        if unique and validated in result:
            raise StateError(f"{label} contains duplicate entry {validated!r}")
        result.append(validated)
    return tuple(result)


def _cache_glob_groups(value: object, label: str) -> tuple[tuple[str, tuple[str, ...]], ...]:
    raw = _as_mapping(value, label)
    result: list[tuple[str, tuple[str, ...]]] = []
    for name in sorted(raw):
        _validate_name(name, f"{label} repository name")
        globs = _string_tuple(raw[name], f"{label}.{name}", _validate_cache_glob)
        if not globs:
            raise StateError(f"{label}.{name} must contain at least one cache glob")
        result.append((name, globs))
    return tuple(result)


def _cache_glob_assignments(
    values: Sequence[str], label: str
) -> tuple[tuple[str, tuple[str, ...]], ...]:
    grouped: dict[str, list[str]] = {}
    for raw in values:
        name, separator, pattern = raw.partition("=")
        if not separator:
            raise Refusal(f"{label} must use NAME=PATH-GLOB: {raw!r}")
        _validate_name(name, f"{label} repository name")
        validated = _validate_cache_glob(pattern)
        entries = grouped.setdefault(name, [])
        if validated in entries:
            raise Refusal(f"duplicate {label} for {name}: {validated!r}")
        entries.append(validated)
    return tuple((name, tuple(grouped[name])) for name in sorted(grouped))


def _cache_globs_for(config: Config, checkout_name: str | None) -> tuple[str, ...]:
    selected = list(config.cache_globs)
    if checkout_name is not None:
        selected.extend(dict(config.repo_cache_globs).get(checkout_name, ()))
    return tuple(dict.fromkeys(selected))


def _as_mapping(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, dict):
        raise StateError(f"{label} must be a JSON object")
    for key in value:
        if not isinstance(key, str):
            raise StateError(f"{label} contains a non-string key")
    return value


def _as_list(value: object, label: str) -> list[object]:
    if not isinstance(value, list):
        raise StateError(f"{label} must be a JSON array")
    return value


def _as_str(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise StateError(f"{label} must be a string")
    return value


def _as_int(value: object, label: str, *, minimum: int = 0) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < minimum:
        raise StateError(f"{label} must be an integer >= {minimum}")
    return value


def _exact_keys(
    value: Mapping[str, object],
    required: AbstractSet[str],
    optional: AbstractSet[str],
    label: str,
) -> None:
    keys = set(value)
    missing = sorted(required - keys)
    unknown = sorted(keys - required - optional)
    if missing or unknown:
        details: list[str] = []
        if missing:
            details.append(f"missing {', '.join(missing)}")
        if unknown:
            details.append(f"unknown {', '.join(unknown)}")
        raise StateError(f"{label} has invalid fields: {'; '.join(details)}")


def _read_json(path: Path, label: str) -> object:
    _refuse_symlink(path, label)
    try:
        with path.open("r", encoding="utf-8") as handle:
            return json.load(handle)
    except FileNotFoundError as exc:
        raise StateError(f"missing {label}: {path}") from exc
    except (OSError, json.JSONDecodeError) as exc:
        raise StateError(f"cannot read {label} {path}: {exc}") from exc


def _refuse_symlink(path: Path, label: str) -> None:
    try:
        if path.is_symlink():
            raise StateError(f"{label} must not be a symlink: {path}")
    except OSError as exc:
        raise StateError(f"cannot inspect {label} {path}: {exc}") from exc


def _relative_inside(root: Path, raw: str, label: str) -> tuple[str, Path]:
    candidate = Path(raw)
    if candidate.is_absolute() or not candidate.parts or ".." in candidate.parts:
        raise Refusal(f"{label} must be a relative path inside {root}: {raw!r}")
    lexical = Path(os.path.normpath(str(candidate)))
    if lexical == Path("."):
        raise Refusal(f"{label} must not be the project root")
    absolute = root / lexical
    _ensure_no_symlink_components(root, absolute, label)
    return lexical.as_posix(), absolute


def _ensure_no_symlink_components(root: Path, path: Path, label: str) -> None:
    try:
        relative = path.relative_to(root)
    except ValueError as exc:
        raise Refusal(f"{label} escapes the project root: {path}") from exc
    current = root
    for part in relative.parts:
        current = current / part
        try:
            if current.is_symlink():
                raise Refusal(f"{label} crosses a symlink: {current}")
        except OSError as exc:
            raise Refusal(f"cannot inspect {label} path {current}: {exc}") from exc


def _ensure_no_mount_components(root: Path, path: Path, label: str) -> None:
    try:
        relative = path.relative_to(root)
    except ValueError as exc:
        raise Refusal(f"{label} escapes its managed root: {path}") from exc
    current = root
    for part in relative.parts:
        current = current / part
        try:
            if os.path.ismount(current):
                raise Refusal(f"{label} crosses a mount point: {current}")
        except OSError as exc:
            raise Refusal(f"cannot inspect {label} mount path {current}: {exc}") from exc


def _mountinfo_path(value: str) -> Path:
    decoded = value
    for escaped, character in (
        ("\\040", " "),
        ("\\011", "\t"),
        ("\\012", "\n"),
        ("\\134", "\\"),
    ):
        decoded = decoded.replace(escaped, character)
    return Path(decoded)


def _assert_no_mountinfo_crossing(root: Path, path: Path, label: str) -> None:
    if not _path_is_within(path, root):
        raise Refusal(f"{label} escapes its managed root: {path}")
    mountinfo_path = Path("/proc/self/mountinfo")
    try:
        lines = mountinfo_path.read_text(
            encoding="utf-8", errors="surrogateescape"
        ).splitlines()
    except OSError as exc:
        raise Refusal(
            f"cannot prove {label} contains no mount points because {mountinfo_path} "
            f"is unreadable: {exc}"
        ) from exc
    for line in lines:
        left, separator, _right = line.partition(" - ")
        if not separator:
            continue
        fields = left.split()
        if len(fields) < 5:
            continue
        mount_point = _mountinfo_path(fields[4])
        if (
            mount_point.is_absolute()
            and mount_point != root
            and _path_is_within(mount_point, root)
            and (
                _path_is_within(path, mount_point)
                or _path_is_within(mount_point, path)
            )
        ):
            raise Refusal(f"{label} crosses or contains a mount point: {mount_point}")


def _path_is_within(candidate: Path, parent: Path) -> bool:
    try:
        candidate.relative_to(parent)
        return True
    except ValueError:
        return False


def _fd_mount_id(fd: int, label: str) -> int:
    path = Path("/proc/self/fdinfo") / str(fd)
    try:
        contents = path.read_text(encoding="ascii")
    except OSError as exc:
        raise Refusal(f"cannot determine mount identity for {label}: {exc}") from exc
    for line in contents.splitlines():
        key, separator, raw = line.partition(":")
        if key == "mnt_id" and separator:
            try:
                value = int(raw.strip())
            except ValueError as exc:
                raise Refusal(f"invalid mount identity for {label}: {raw!r}") from exc
            if value > 0:
                return value
    raise Refusal(f"cannot determine mount identity for {label}: mnt_id is absent")


def _open_directory_identity(path: Path, label: str) -> tuple[int, int, int]:
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise Refusal(f"cannot safely open {label} {path}: {exc}") from exc
    try:
        metadata = os.fstat(fd)
        return metadata.st_dev, metadata.st_ino, _fd_mount_id(fd, str(path))
    finally:
        os.close(fd)


def _config_payload(
    worktrees_dir: str,
    machine: str,
    default_remote: str,
    default_landed_ref: str,
    heartbeat_ttl_seconds: int,
    liveness_command: str,
    max_active_slots: int | None = None,
    layout: str = "nested",
    cache_globs: Sequence[str] = (),
    repo_cache_globs: Sequence[tuple[str, Sequence[str]]] = (),
    post_provision_hooks: Sequence[str] = (),
    disk_advisory_bytes: int | None = None,
    disk_provisioning_floor_bytes: int | None = None,
    disk_emergency_bytes: int | None = None,
) -> dict[str, object]:
    payload: dict[str, object] = {
        "schema": SCHEMA,
        "worktrees_dir": worktrees_dir,
        "machine": machine,
        "default_remote": default_remote,
        "default_landed_ref": default_landed_ref,
        "heartbeat_ttl_seconds": heartbeat_ttl_seconds,
        "liveness_command": liveness_command,
    }
    # Absent means "no cap", so an unset cap must not appear in the payload at
    # all; writing an explicit null would make init non-idempotent against every
    # configuration written before the field existed.
    if max_active_slots is not None:
        payload["max_active_slots"] = max_active_slots
    if layout != "nested":
        payload["layout"] = layout
    if cache_globs:
        payload["cache_globs"] = list(cache_globs)
    if repo_cache_globs:
        payload["repo_cache_globs"] = {
            name: list(patterns) for name, patterns in repo_cache_globs
        }
    if post_provision_hooks:
        payload["post_provision_hooks"] = list(post_provision_hooks)
    if disk_advisory_bytes is not None:
        payload["disk_advisory_bytes"] = disk_advisory_bytes
        payload["disk_provisioning_floor_bytes"] = disk_provisioning_floor_bytes
        payload["disk_emergency_bytes"] = disk_emergency_bytes
    return payload


def _canonical_config_payload(value: Mapping[str, object]) -> dict[str, object]:
    canonical = dict(value)
    if canonical.get("layout") == "nested":
        canonical.pop("layout")
    if canonical.get("cache_globs") == []:
        canonical.pop("cache_globs")
    if canonical.get("repo_cache_globs") == {}:
        canonical.pop("repo_cache_globs")
    if canonical.get("post_provision_hooks") == []:
        canonical.pop("post_provision_hooks")
    return canonical


def _repaired_config(
    existing: dict[str, object], payload: dict[str, object]
) -> tuple[dict[str, object], list[str]]:
    """Bring a configuration written by an older build up to this schema.

    Repair is deliberately narrow. It ADDS fields this build requires and that
    the file does not have, and it bumps ``schema``. It never rewrites a field
    that is present with a different value, because a configuration is a
    description of where live state already is: silently changing
    ``worktrees_dir`` or ``machine`` would not fix a stale file, it would point
    the tool at a different registry and orphan every slot recorded in the old
    one. Every conflicting field is refused, and all of them are listed at once,
    so a repair can never be a disguised relocation and one fix per run is not
    the operator's only option.

    Returns the repaired mapping and a human-readable list of what changed, so
    the operator sees the migration rather than trusting it.
    """
    repaired = dict(existing)
    changes: list[str] = []
    conflicts: list[str] = []
    for key, want in payload.items():
        if key not in repaired:
            if key == "layout" and want == "flat":
                conflicts.append(
                    "layout: absent means nested; repair will not reinterpret existing "
                    "storage as flat"
                )
                continue
            repaired[key] = want
            changes.append(f"added {key}={want!r}")
            continue
        have = repaired[key]
        if have == want:
            continue
        if key in _CONFIG_REPAIR_UPDATABLE:
            # `schema` is what repair exists to advance. `max_active_slots` is a
            # policy knob, not a pointer to state: changing it re-tunes future
            # allocation and cannot orphan a slot that already exists. Both are
            # still reported, so an update is never silent.
            repaired[key] = want
            changes.append(f"{key} {have!r} -> {want!r}")
            continue
        conflicts.append(f"{key}: file has {have!r}, command line says {want!r}")
    unknown = sorted(set(repaired) - set(payload) - OPTIONAL_CONFIG_KEYS)
    if unknown:
        conflicts.append(
            "unknown field(s) this build does not understand: " + ", ".join(unknown)
        )
    if conflicts:
        raise Refusal(
            "cannot repair configuration without changing a field that already "
            "has a value; resolve these by hand and rerun:\n  "
            + "\n  ".join(conflicts)
        )
    if not changes:
        changes.append("no field needed repair")
    return repaired, changes


def _config_is_authoritative_candidate(root: Path, config_path: Path) -> bool:
    if config_path.is_symlink():
        return True
    try:
        raw = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return True
    if not isinstance(raw, dict):
        return True
    worktrees_value = raw.get("worktrees_dir")
    layout_value = raw.get("layout", "nested")
    if not isinstance(worktrees_value, str) or layout_value not in {"nested", "flat"}:
        return True
    candidate = Path(worktrees_value)
    if candidate.is_absolute() or not candidate.parts or ".." in candidate.parts:
        return True
    worktrees = root / Path(os.path.normpath(str(candidate)))
    control = worktrees if layout_value == "nested" else worktrees.parent
    return (
        (control / "wrkslots").is_symlink()
        or any(control.glob("ACTIVE.*.json"))
        or any(control.glob("ARCHIVED.*.json"))
        or any(control.glob("ACTIVE.*.journal"))
        or any(control.glob("EVENTS.*"))
    )


def _discover_root(explicit: str | None) -> Path:
    if explicit is not None:
        candidate = Path(explicit).absolute()
        if candidate.is_symlink() or not candidate.is_dir():
            raise Refusal(f"project root must be an existing real directory: {candidate}")
        return candidate.resolve(strict=True)
    current = Path.cwd().resolve(strict=True)
    fallback: Path | None = None
    for candidate in (current, *current.parents):
        config_path = candidate / CONFIG_NAME
        if config_path.exists() or config_path.is_symlink():
            fallback = candidate
            if _config_is_authoritative_candidate(candidate, config_path):
                return candidate
    if fallback is not None:
        return fallback
    raise Refusal(f"no {CONFIG_NAME} found; run 'wrkslots init' from the project root")


def _load_config(explicit_root: str | None, machine_override: str | None) -> Config:
    root = _discover_root(explicit_root)
    path = root / CONFIG_NAME
    raw = _as_mapping(_read_json(path, "configuration"), "configuration")
    required = {
        "schema",
        "worktrees_dir",
        "machine",
        "default_remote",
        "default_landed_ref",
        "heartbeat_ttl_seconds",
        "liveness_command",
    }
    try:
        _exact_keys(raw, required, OPTIONAL_CONFIG_KEYS, "configuration")
    except StateError as exc:
        # A configuration written by an older build is missing fields this build
        # requires. Without a repair path every command, including read-only
        # status, is dead with no way back; say where the way back is.
        raise StateError(
            f"{exc} in {path}. Migrate it with `wrkslots init --repair` "
            f"(it refuses to change any field that already has a conflicting "
            f"value)."
        ) from exc
    if _as_int(raw["schema"], "configuration.schema") != SCHEMA:
        raise StateError(
            f"unsupported configuration schema in {path}: found "
            f"{raw['schema']!r}, this build speaks {SCHEMA}. Migrate it with "
            f"`wrkslots init --repair` (it refuses to change any field that "
            f"already has a conflicting value)."
        )
    machine = machine_override or os.environ.get("WRKSLOTS_MACHINE")
    if machine is None:
        machine = _as_str(raw["machine"], "configuration.machine")
    _validate_name(machine, "machine")
    relative, worktrees = _relative_inside(
        root, _as_str(raw["worktrees_dir"], "configuration.worktrees_dir"), "worktrees_dir"
    )
    del relative
    if not worktrees.is_dir() or worktrees.is_symlink():
        raise StateError(f"configured worktrees directory is missing or unsafe: {worktrees}")
    layout = _validate_layout(
        _as_str(raw.get("layout", "nested"), "configuration.layout")
    )
    control = _control_directory_for(worktrees, layout)
    if not control.is_dir() or control.is_symlink():
        raise StateError(f"configured control directory is missing or unsafe: {control}")
    remote = _validate_remote(
        _as_str(raw["default_remote"], "configuration.default_remote")
    )
    landed_ref = _validate_full_ref(
        _as_str(raw["default_landed_ref"], "configuration.default_landed_ref"),
        "default landed ref",
    )
    liveness_relative, liveness_command = _relative_inside(
        root,
        _as_str(raw["liveness_command"], "configuration.liveness_command"),
        "liveness command",
    )
    del liveness_relative
    if (
        not liveness_command.is_file()
        or liveness_command.is_symlink()
        or (
            liveness_command.suffix != ".py"
            and not os.access(liveness_command, os.X_OK)
        )
    ):
        raise StateError(
            f"configured liveness command is missing, not runnable, or unsafe: {liveness_command}"
        )
    cap = raw.get("max_active_slots")
    cache_globs = (
        ()
        if "cache_globs" not in raw
        else _string_tuple(
            raw["cache_globs"], "configuration.cache_globs", _validate_cache_glob
        )
    )
    repo_cache_globs = (
        ()
        if "repo_cache_globs" not in raw
        else _cache_glob_groups(
            raw["repo_cache_globs"], "configuration.repo_cache_globs"
        )
    )
    post_provision_hooks = (
        ()
        if "post_provision_hooks" not in raw
        else _string_tuple(
            raw["post_provision_hooks"],
            "configuration.post_provision_hooks",
            _validate_hook,
            unique=False,
        )
    )
    disk_keys = (
        "disk_advisory_bytes",
        "disk_provisioning_floor_bytes",
        "disk_emergency_bytes",
    )
    present_disk_keys = [key for key in disk_keys if key in raw]
    if present_disk_keys and len(present_disk_keys) != len(disk_keys):
        raise StateError(
            "configuration disk thresholds must supply advisory, provisioning floor, "
            "and emergency bytes together"
        )
    disk_values: tuple[int | None, int | None, int | None]
    if present_disk_keys:
        advisory = _as_int(raw[disk_keys[0]], f"configuration.{disk_keys[0]}", minimum=1)
        provisioning = _as_int(
            raw[disk_keys[1]], f"configuration.{disk_keys[1]}", minimum=1
        )
        emergency = _as_int(raw[disk_keys[2]], f"configuration.{disk_keys[2]}", minimum=1)
        if not emergency < provisioning < advisory:
            raise StateError(
                "configuration disk thresholds must satisfy "
                "emergency < provisioning floor < advisory"
            )
        disk_values = advisory, provisioning, emergency
    else:
        disk_values = None, None, None
    return Config(
        root=root,
        config_path=path,
        worktrees=worktrees,
        control=control,
        machine=machine,
        default_remote=remote,
        default_landed_ref=landed_ref,
        heartbeat_ttl_seconds=_as_int(
            raw["heartbeat_ttl_seconds"],
            "configuration.heartbeat_ttl_seconds",
            minimum=1,
        ),
        liveness_command=liveness_command,
        max_active_slots=(
            None
            if cap is None
            else _as_int(cap, "configuration.max_active_slots", minimum=0)
        ),
        layout=layout,
        cache_globs=cache_globs,
        repo_cache_globs=repo_cache_globs,
        post_provision_hooks=post_provision_hooks,
        disk_advisory_bytes=disk_values[0],
        disk_provisioning_floor_bytes=disk_values[1],
        disk_emergency_bytes=disk_values[2],
    )


def _active_path(config: Config, machine: str | None = None) -> Path:
    selected = machine or config.machine
    _validate_name(selected, "machine")
    return config.control / f"ACTIVE.{selected}.json"


def _landed_ref_for_remote(config: Config, remote: str) -> str:
    prefix = f"refs/remotes/{config.default_remote}/"
    if not config.default_landed_ref.startswith(prefix):
        raise StateError(f"configured landed ref must be under {prefix}")
    return f"refs/remotes/{remote}/{config.default_landed_ref.removeprefix(prefix)}"


def _archive_path(config: Config, machine: str | None = None) -> Path:
    selected = machine or config.machine
    _validate_name(selected, "machine")
    return config.control / f"ARCHIVED.{selected}.json"


def _journal_path(config: Config, machine: str | None = None) -> Path:
    selected = machine or config.machine
    _validate_name(selected, "machine")
    return config.control / f"ACTIVE.{selected}.journal"


def _hold_path(config: Config, slot: str, machine: str | None = None) -> Path:
    selected = machine or config.machine
    _validate_name(selected, "machine")
    _validate_name(slot, "slot")
    return config.control / f"HOLD.{len(selected)}.{selected}.{slot}.json"


def _load_hold_snapshot(
    config: Config, slot: str, machine: str | None = None
) -> dict[str, object] | None:
    selected = machine or config.machine
    path = _hold_path(config, slot, selected)
    if not path.exists() and not path.is_symlink():
        return None
    raw = _as_mapping(_read_json(path, "slot hold"), "slot hold")
    _exact_keys(raw, {"schema", "machine", "slot", "held_at", "reason"}, set(), "slot hold")
    if _as_int(raw["schema"], "slot hold.schema") != HOLD_SCHEMA:
        raise StateError(f"unsupported slot hold schema in {path}")
    if _as_str(raw["machine"], "slot hold.machine") != selected:
        raise StateError(f"slot hold machine does not match its filename: {path}")
    if _as_str(raw["slot"], "slot hold.slot") != slot:
        raise StateError(f"slot hold name does not match its filename: {path}")
    _parse_timestamp(_as_str(raw["held_at"], "slot hold.held_at"), "slot hold.held_at")
    if not _as_str(raw["reason"], "slot hold.reason").strip():
        raise StateError(f"slot hold has an empty reason: {path}")
    return dict(raw)


def _load_hold(
    config: Config, slot: str, machine: str | None = None
) -> dict[str, object] | None:
    selected = machine or config.machine
    events = _load_events(config, selected)
    if not events:
        return _load_hold_snapshot(config, slot, selected)
    hold: dict[str, object] | None = None
    for event in events:
        payload = _as_mapping(event["payload"], "append-only event.payload")
        if event.get("kind") == "state-imported":
            for item in _as_list(payload.get("holds", []), "imported holds"):
                candidate = dict(_as_mapping(item, "imported hold"))
                if candidate.get("slot") == slot:
                    hold = candidate
        elif payload.get("slot") == slot and event.get("kind") == "slot-held":
            hold = {
                "schema": HOLD_SCHEMA,
                "machine": selected,
                "slot": slot,
                "held_at": event["recorded_at"],
                "reason": payload.get("reason"),
            }
        elif payload.get("slot") == slot and event.get("kind") == "slot-hold-released":
            hold = None
    return hold


def _assert_not_held(config: Config, record: ActiveRecord) -> None:
    hold = _load_hold(config, record.slot, record.machine)
    if hold is not None:
        raise Refusal(
            f"slot {record.slot} is held: {_as_str(hold['reason'], 'slot hold.reason')}; "
            f"run 'wrkslots unhold {record.slot}' only when the hold is no longer needed"
        )


def _lock_path(subject: Path) -> Path:
    if subject.name == "ACTIVE":
        return subject.parent / "ACTIVE.global.lock"
    return subject.with_name(f"{subject.name}.lock")


@contextlib.contextmanager
def _locked(subject: Path, *, exclusive: bool, wait_seconds: float) -> Iterator[None]:
    path = _lock_path(subject)
    if path.parent.is_symlink() or not path.parent.is_dir():
        raise Refusal(f"unsafe lock parent: {path.parent}")
    try:
        fd = os.open(
            path,
            os.O_RDWR | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW,
            0o600,
        )
    except OSError as exc:
        raise Refusal(f"cannot open state lock {path}: {exc}") from exc
    try:
        mode = os.fstat(fd).st_mode
    except OSError as exc:
        os.close(fd)
        raise Refusal(f"cannot inspect state lock {path}: {exc}") from exc
    if not stat.S_ISREG(mode):
        os.close(fd)
        raise Refusal(f"state lock is not a regular file: {path}")
    operation = fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH
    deadline = time.monotonic() + wait_seconds
    try:
        while True:
            try:
                fcntl.flock(fd, operation | fcntl.LOCK_NB)
                break
            except BlockingIOError as exc:
                if time.monotonic() >= deadline:
                    raise Refusal(
                        f"state lock is busy for {subject}; retry after the current command exits"
                    ) from exc
                time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
        yield
    finally:
        try:
            fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)


@contextlib.contextmanager
def _locked_config(path: Path, wait_seconds: float) -> Iterator[int]:
    lock_path = path.parent
    try:
        fd = os.open(
            lock_path,
            os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW,
        )
    except OSError as exc:
        raise Refusal(f"cannot open configuration lock {lock_path}: {exc}") from exc
    try:
        mode = os.fstat(fd).st_mode
    except OSError as exc:
        os.close(fd)
        raise Refusal(f"cannot inspect configuration lock {lock_path}: {exc}") from exc
    if not stat.S_ISDIR(mode):
        os.close(fd)
        raise Refusal(f"configuration lock is not a directory: {lock_path}")
    deadline = time.monotonic() + wait_seconds
    try:
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError as exc:
                if time.monotonic() >= deadline:
                    raise Refusal(
                        f"configuration lock is busy for {lock_path}; retry after the "
                        "other wrkslots mutation exits"
                    ) from exc
                time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
        yield fd
    finally:
        try:
            fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)


@contextlib.contextmanager
def _mutation_locks(config: Config, wait_seconds: float) -> Iterator[None]:
    with _locked_config(config.config_path, wait_seconds):
        current = _load_config(str(config.root), config.machine)
        if current != config:
            raise Refusal(
                "configuration changed while this command was starting; rerun so it "
                "uses one coherent policy"
            )
        global_subject = config.control / "ACTIVE"
        with _locked(global_subject, exclusive=True, wait_seconds=wait_seconds):
            with _locked(_active_path(config), exclusive=True, wait_seconds=wait_seconds):
                yield


def _fsync_directory(directory: Path) -> None:
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def _atomic_write_json(path: Path, payload: object) -> None:
    _refuse_symlink(path, "state file")
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise Refusal(f"cannot prepare state directory {path.parent}: {exc}") from exc
    prefix = f"{path.name}.tmp."
    leftovers = sorted(path.parent.glob(f"{prefix}*"))
    if leftovers:
        raise StateError(
            f"partial state update exists for {path.name}: {leftovers[0]}; "
            "inspect it and use 'wrkslots recover'"
        )
    temp = path.parent / f"{prefix}{os.getpid()}.{uuid.uuid4().hex}"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC
    try:
        fd = os.open(temp, flags, 0o644)
    except OSError as exc:
        raise Refusal(f"cannot create temporary state file {temp}: {exc}") from exc
    try:
        with os.fdopen(fd, "w", encoding="utf-8", closefd=True) as handle:
            json.dump(payload, handle, indent=2, sort_keys=True, ensure_ascii=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp, path)
        _fsync_directory(path.parent)
    except OSError as exc:
        with contextlib.suppress(FileNotFoundError):
            temp.unlink()
        raise Refusal(f"cannot durably replace state file {path}: {exc}") from exc
    except BaseException:
        with contextlib.suppress(FileNotFoundError):
            temp.unlink()
        raise


def _json_equal(left: object, right: object) -> bool:
    return json.dumps(
        left, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ) == json.dumps(right, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def _recover_config_write(path: Path, payload: object) -> None:
    leftovers = sorted(path.parent.glob(f"{path.name}.tmp.*"))
    if not leftovers:
        return
    if path.exists() or path.is_symlink():
        existing = _as_mapping(_read_json(path, "configuration"), "configuration")
        if dict(existing) != payload:
            raise StateError(
                f"configuration temp file exists beside a different durable configuration: {leftovers[0]}"
            )
        for leftover in leftovers:
            leftover.unlink()
        _fsync_directory(path.parent)
        return
    matching: list[Path] = []
    for leftover in leftovers:
        try:
            candidate = _read_json(leftover, "configuration temp file")
        except StateError:
            leftover.unlink()
            continue
        if candidate != payload:
            raise StateError(
                f"configuration temp file records different init input: {leftover}"
            )
        matching.append(leftover)
    if len(matching) > 1:
        raise StateError("multiple complete configuration temp files require inspection")
    if matching:
        os.replace(matching[0], path)
        _fsync_directory(path.parent)
    else:
        _fsync_directory(path.parent)


def _remove_control_file(path: Path) -> None:
    _refuse_symlink(path, "control file")
    try:
        path.unlink()
        _fsync_directory(path.parent)
    except OSError as exc:
        raise Refusal(f"cannot remove control file {path}: {exc}") from exc


def _identity_to_obj(owner: ProcessIdentity | None) -> object:
    if owner is None:
        return None
    return {
        "pid": owner.pid,
        "start_ticks": owner.start_ticks,
        "boot_id": owner.boot_id,
        "host_id": owner.host_id,
        "cgroup_path": owner.cgroup_path,
    }


def _identity_from_obj(value: object, label: str) -> ProcessIdentity | None:
    if value is None:
        return None
    raw = _as_mapping(value, label)
    _exact_keys(
        raw,
        {"pid", "start_ticks", "boot_id", "host_id", "cgroup_path"},
        set(),
        label,
    )
    identity = ProcessIdentity(
        pid=_as_int(raw["pid"], f"{label}.pid", minimum=1),
        start_ticks=_as_int(raw["start_ticks"], f"{label}.start_ticks", minimum=1),
        boot_id=_as_str(raw["boot_id"], f"{label}.boot_id"),
        host_id=_as_str(raw["host_id"], f"{label}.host_id"),
        cgroup_path=_as_str(raw["cgroup_path"], f"{label}.cgroup_path"),
    )
    if (
        not identity.boot_id
        or not identity.host_id
        or not identity.cgroup_path.startswith("/")
    ):
        raise StateError(f"{label} has an invalid boot, machine, or cgroup identity")
    return identity


def _handoff_to_obj(handoff: Handoff | None) -> object:
    if handoff is None:
        return None
    return {
        "recorded_at": handoff.recorded_at,
        "validation": list(handoff.validation),
        "limitations": list(handoff.limitations),
        "continuation": handoff.continuation,
    }


def _handoff_from_obj(value: object, label: str) -> Handoff | None:
    if value is None:
        return None
    raw = _as_mapping(value, label)
    _exact_keys(
        raw,
        {"recorded_at", "validation", "limitations", "continuation"},
        set(),
        label,
    )
    recorded_at = _as_str(raw["recorded_at"], f"{label}.recorded_at")
    _parse_timestamp(recorded_at, f"{label}.recorded_at")
    validation = tuple(
        _as_str(item, f"{label}.validation[{index}]")
        for index, item in enumerate(_as_list(raw["validation"], f"{label}.validation"))
    )
    limitations = tuple(
        _as_str(item, f"{label}.limitations[{index}]")
        for index, item in enumerate(_as_list(raw["limitations"], f"{label}.limitations"))
    )
    continuation = _as_str(raw["continuation"], f"{label}.continuation")
    if not validation or not continuation:
        raise StateError(f"{label} requires validation evidence and an exact continuation")
    return Handoff(recorded_at, validation, limitations, continuation)


def _import_source_to_obj(source: _ImportSource | None) -> object:
    if source is None:
        return None
    return {
        "format": source.format,
        "path": source.path,
        "file_sha256": source.file_sha256,
        "row_sha256": source.row_sha256,
        "status": source.status,
        "row": dict(source.row),
    }


def _import_source_from_obj(value: object, label: str) -> _ImportSource | None:
    if value is None:
        return None
    raw = _as_mapping(value, label)
    _exact_keys(
        raw,
        {"format", "path", "file_sha256", "row_sha256", "status", "row"},
        set(),
        label,
    )
    source_format = _as_str(raw["format"], f"{label}.format")
    if source_format != "worktree-state-v3":
        raise StateError(f"{label}.format is unsupported: {source_format!r}")
    path = _as_str(raw["path"], f"{label}.path")
    if Path(path).is_absolute() or not Path(path).parts or ".." in Path(path).parts:
        raise StateError(f"{label}.path must be project-relative")
    file_sha256 = _as_str(raw["file_sha256"], f"{label}.file_sha256")
    row_sha256 = _as_str(raw["row_sha256"], f"{label}.row_sha256")
    if not re.fullmatch(r"[0-9a-f]{64}", file_sha256) or not re.fullmatch(
        r"[0-9a-f]{64}", row_sha256
    ):
        raise StateError(f"{label} digests must be lowercase SHA-256 values")
    status = _as_str(raw["status"], f"{label}.status")
    if status not in HISTORICAL_ACTIVE_STATUSES:
        raise StateError(f"{label}.status is not an active historical state")
    row = dict(_as_mapping(raw["row"], f"{label}.row"))
    if _event_digest(row) != row_sha256:
        raise StateError(f"{label}.row digest does not match its content")
    if row.get("status") != status:
        raise StateError(f"{label}.status does not match its source row")
    return _ImportSource(source_format, path, file_sha256, row_sha256, status, row)


def _checkout_to_obj(checkout: Checkout) -> dict[str, object]:
    return {
        "name": checkout.name,
        "path": checkout.path,
        "repository": checkout.repository,
        "branch": checkout.branch,
        "start_point": checkout.start_point,
        "remote": checkout.remote,
        "remote_url_sha256": checkout.remote_url_sha256,
        "landed_ref": checkout.landed_ref,
        "head": checkout.head,
        "containing_remote_refs": list(checkout.containing_remote_refs),
        "vcs": checkout.vcs,
    }


def _checkout_from_obj(value: object, label: str) -> Checkout:
    raw = _as_mapping(value, label)
    fields = {
        "name",
        "path",
        "repository",
        "branch",
        "start_point",
        "remote",
        "remote_url_sha256",
        "landed_ref",
        "head",
        "containing_remote_refs",
        "vcs",
    }
    _exact_keys(raw, fields, set(), label)
    checkout = Checkout(
        name=_as_str(raw["name"], f"{label}.name"),
        path=_as_str(raw["path"], f"{label}.path"),
        repository=_as_str(raw["repository"], f"{label}.repository"),
        branch=_as_str(raw["branch"], f"{label}.branch"),
        start_point=_as_str(raw["start_point"], f"{label}.start_point"),
        remote=_as_str(raw["remote"], f"{label}.remote"),
        remote_url_sha256=_as_str(
            raw["remote_url_sha256"], f"{label}.remote_url_sha256"
        ),
        landed_ref=_as_str(raw["landed_ref"], f"{label}.landed_ref"),
        head=_as_str(raw["head"], f"{label}.head"),
        containing_remote_refs=tuple(
            _as_str(item, f"{label}.containing_remote_refs[{index}]")
            for index, item in enumerate(
                _as_list(
                    raw["containing_remote_refs"],
                    f"{label}.containing_remote_refs",
                )
            )
        ),
        vcs=_as_str(raw["vcs"], f"{label}.vcs"),
    )
    _validate_name(checkout.name, "checkout name")
    _validate_remote(checkout.remote)
    if not re.fullmatch(r"[0-9a-f]{64}", checkout.remote_url_sha256):
        raise StateError(f"{label}.remote_url_sha256 must be a lowercase SHA-256")
    _validate_ref(checkout.branch, "branch")
    if not SHA_RE.fullmatch(checkout.start_point):
        raise StateError(f"{label}.start_point must be a full lowercase Git commit")
    _validate_full_ref(checkout.landed_ref, "landed ref")
    if checkout.vcs != "git":
        raise StateError(f"unsupported VCS in {label}: {checkout.vcs!r}")
    if not SHA_RE.fullmatch(checkout.head):
        raise StateError(f"{label}.head must be a full lowercase Git commit")
    if any(
        not ref.startswith(f"refs/remotes/{checkout.remote}/")
        for ref in checkout.containing_remote_refs
    ):
        raise StateError(f"{label}.containing_remote_refs names an unauthorized ref")
    return checkout


def _record_to_obj(record: ActiveRecord) -> dict[str, object]:
    value: dict[str, object] = {
        "slot": record.slot,
        "agent": record.agent,
        "task": record.task,
        "purpose": record.purpose,
        "slot_type": record.slot_type,
        "machine": record.machine,
        "generation": record.generation,
        "created_at": record.created_at,
        "heartbeat_at": record.heartbeat_at,
        "heartbeat_ttl_seconds": record.heartbeat_ttl_seconds,
        "owner": _identity_to_obj(record.owner),
        "coordinator_lease": _identity_to_obj(record.coordinator_lease),
        "coordinator_recovery_note": record.coordinator_recovery_note,
        "handoff": _handoff_to_obj(record.handoff),
        "checkouts": [_checkout_to_obj(item) for item in record.checkouts],
    }
    if record.layout is not None:
        value["layout"] = record.layout
    if record.import_source is not None:
        value["import_source"] = _import_source_to_obj(record.import_source)
    return value


def _record_from_obj(value: object, label: str) -> ActiveRecord:
    raw = _as_mapping(value, label)
    required_fields = {
        "slot",
        "agent",
        "task",
        "purpose",
        "machine",
        "generation",
        "created_at",
        "heartbeat_at",
        "heartbeat_ttl_seconds",
        "owner",
        "coordinator_lease",
        "coordinator_recovery_note",
        "handoff",
        "checkouts",
    }
    _exact_keys(raw, required_fields, {"slot_type", "layout", "import_source"}, label)
    checkouts = tuple(
        _checkout_from_obj(item, f"{label}.checkouts[{index}]")
        for index, item in enumerate(_as_list(raw["checkouts"], f"{label}.checkouts"))
    )
    coordinator_lease = _identity_from_obj(
        raw["coordinator_lease"], f"{label}.coordinator_lease"
    )
    if coordinator_lease is None:
        raise StateError(f"{label} has no coordinator lease")
    record = ActiveRecord(
        slot=_as_str(raw["slot"], f"{label}.slot"),
        agent=_as_str(raw["agent"], f"{label}.agent"),
        task=_as_str(raw["task"], f"{label}.task"),
        purpose=_as_str(raw["purpose"], f"{label}.purpose"),
        slot_type=_as_str(raw.get("slot_type", "agent"), f"{label}.slot_type"),
        machine=_as_str(raw["machine"], f"{label}.machine"),
        generation=_as_int(raw["generation"], f"{label}.generation", minimum=1),
        created_at=_as_str(raw["created_at"], f"{label}.created_at"),
        heartbeat_at=_as_str(raw["heartbeat_at"], f"{label}.heartbeat_at"),
        heartbeat_ttl_seconds=_as_int(
            raw["heartbeat_ttl_seconds"], f"{label}.heartbeat_ttl_seconds", minimum=1
        ),
        owner=_identity_from_obj(raw["owner"], f"{label}.owner"),
        coordinator_lease=coordinator_lease,
        coordinator_recovery_note=(
            None
            if raw["coordinator_recovery_note"] is None
            else _as_str(
                raw["coordinator_recovery_note"],
                f"{label}.coordinator_recovery_note",
            )
        ),
        handoff=_handoff_from_obj(raw["handoff"], f"{label}.handoff"),
        checkouts=checkouts,
        layout=(
            None
            if "layout" not in raw
            else _validate_layout(_as_str(raw["layout"], f"{label}.layout"))
        ),
        import_source=_import_source_from_obj(
            raw.get("import_source"), f"{label}.import_source"
        ),
    )
    _validate_name(record.slot, "slot")
    _validate_name(record.agent, "agent")
    _validate_name(record.machine, "machine")
    if not record.task or not record.purpose:
        raise StateError(f"{label} has an empty task or purpose")
    if record.slot_type not in SLOT_TYPES:
        raise StateError(
            f"{label}.slot_type must be one of {', '.join(SLOT_TYPES)}"
        )
    _parse_timestamp(record.created_at, f"{label}.created_at")
    _parse_timestamp(record.heartbeat_at, f"{label}.heartbeat_at")
    if not record.checkouts and record.import_source is None:
        raise StateError(f"{label} has no checkouts")
    if not record.checkouts and record.layout != "nested":
        raise StateError(
            f"{label} has no checkouts but is not a historical nested slot"
        )
    if record.coordinator_recovery_note is not None and record.owner is not None:
        raise StateError(f"{label} has coordinator recovery evidence for a bound owner")
    names = [item.name for item in record.checkouts]
    if len(names) != len(set(names)):
        raise StateError(f"{label} has duplicate checkout names")
    if record.import_source is not None:
        _assert_import_source_matches_record(record, label)
    return record


def _assert_import_source_matches_record(record: ActiveRecord, label: str) -> None:
    source = record.import_source
    assert source is not None
    if record.slot_type != "agent" or record.layout != "nested" or record.owner is None:
        raise StateError(
            f"{label} historical import must be a nested agent slot with an exact owner"
        )
    row = source.row
    expected_task, expected_purpose = _historical_task_and_purpose(
        row, f"{label}.import_source.row", record.agent
    )
    if expected_task != record.task or expected_purpose != record.purpose:
        raise StateError(f"{label} differs from its historical task or purpose")
    if row.get("allocated") != record.created_at:
        raise StateError(f"{label} differs from its historical allocation time")
    sidecar = _as_mapping(
        row.get("owner_sidecar"), f"{label}.import_source.row.owner_sidecar"
    )
    expected_owner = (
        sidecar.get("supervisor_pid"),
        sidecar.get("start_ticks"),
        sidecar.get("boot_id"),
        sidecar.get("cgroup_path"),
    )
    actual_owner = (
        record.owner.pid,
        record.owner.start_ticks,
        record.owner.boot_id,
        record.owner.cgroup_path,
    )
    if expected_owner != actual_owner:
        raise StateError(f"{label} owner differs from its historical owner_sidecar")
    if sidecar.get("slot") != record.slot or sidecar.get("agent") != record.agent:
        raise StateError(f"{label} identity differs from its historical owner_sidecar")
    for checkout in record.checkouts:
        if row.get(f"{checkout.name}_path") != checkout.path:
            raise StateError(
                f"{label} checkout {checkout.name} differs from its historical path"
            )


def _active_from_obj(
    config: Config, value: object, selected: str, label: str
) -> ActiveState:
    raw = _as_mapping(value, label)
    _exact_keys(raw, {"schema", "machine", "revision", "slots"}, set(), "active state")
    if _as_int(raw["schema"], "active state.schema") != SCHEMA:
        raise StateError(f"unsupported active state schema in {label}")
    actual_machine = _as_str(raw["machine"], "active state.machine")
    if actual_machine != selected:
        raise StateError(
            f"active state machine mismatch: filename says {selected}, content says {actual_machine}"
        )
    slots = tuple(
        _record_from_obj(item, f"active state.slots[{index}]")
        for index, item in enumerate(_as_list(raw["slots"], "active state.slots"))
    )
    slot_names = [item.slot for item in slots]
    agents = [
        item.agent
        for item in slots
        if item.slot_type == "agent" and item.import_source is None
    ]
    if len(slot_names) != len(set(slot_names)):
        raise StateError(f"duplicate slot in {label}")
    if len(agents) != len(set(agents)):
        raise StateError(f"one agent owns more than one active slot in {label}")
    for record in slots:
        if record.machine != selected:
            raise StateError(
                f"slot {record.slot} says machine {record.machine}, expected shard {selected}"
            )
        _assert_record_paths(config, record)
    return ActiveState(
        machine=actual_machine,
        revision=_as_int(raw["revision"], "active state.revision"),
        slots=slots,
    )


def _load_active_snapshot(config: Config, machine: str | None = None) -> ActiveState:
    selected = machine or config.machine
    path = _active_path(config, selected)
    return _active_from_obj(config, _read_json(path, "active state"), selected, str(path))


def _active_to_obj(state: ActiveState) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "machine": state.machine,
        "revision": state.revision,
        "slots": [_record_to_obj(item) for item in sorted(state.slots, key=lambda x: x.slot)],
    }


def _archive_from_obj(
    config: Config, value: object, selected: str, label_prefix: str
) -> ArchiveState:
    raw = _as_mapping(value, label_prefix)
    _exact_keys(raw, {"schema", "machine", "revision", "records"}, set(), "archive state")
    if _as_int(raw["schema"], "archive state.schema") != SCHEMA:
        raise StateError(f"unsupported archive state schema in {label_prefix}")
    actual_machine = _as_str(raw["machine"], "archive state.machine")
    if actual_machine != selected:
        raise StateError(
            f"archive state machine mismatch: filename says {selected}, content says {actual_machine}"
        )
    records: list[dict[str, object]] = []
    archive_ids: set[str] = set()
    archived_slots: set[str] = set()
    for index, item in enumerate(_as_list(raw["records"], "archive state.records")):
        mapping = _as_mapping(item, f"archive state.records[{index}]")
        label = f"archive state.records[{index}]"
        fields = {
            "archive_id",
            "slot",
            "agent",
            "task",
            "purpose",
            "machine",
            "generation",
            "created_at",
            "finished_at",
            "mode",
            "actor",
            "physical_storage",
            "validation",
            "limitations",
            "continuation",
            "checkouts",
        }
        _exact_keys(
            mapping,
            fields,
            {"slot_type", "salvage", "layout", "import_source"},
            label,
        )
        archive_id = _as_str(mapping["archive_id"], f"{label}.archive_id")
        if archive_id in archive_ids:
            raise StateError(f"duplicate archive_id in {label_prefix}: {archive_id}")
        archive_ids.add(archive_id)
        slot = _validate_name(_as_str(mapping["slot"], f"{label}.slot"), "archived slot")
        if slot in archived_slots:
            raise StateError(f"duplicate archived slot in {label_prefix}: {slot}")
        archived_slots.add(slot)
        agent = _validate_name(
            _as_str(mapping["agent"], f"{label}.agent"), "archived agent"
        )
        if not _as_str(mapping["task"], f"{label}.task") or not _as_str(
            mapping["purpose"], f"{label}.purpose"
        ):
            raise StateError(f"{label} has an empty task or purpose")
        slot_type = _as_str(mapping.get("slot_type", "agent"), f"{label}.slot_type")
        if slot_type not in SLOT_TYPES:
            raise StateError(f"{label}.slot_type must be one of {', '.join(SLOT_TYPES)}")
        record_layout = (
            config.layout
            if "layout" not in mapping
            else _validate_layout(_as_str(mapping["layout"], f"{label}.layout"))
        )
        import_source = _import_source_from_obj(
            mapping.get("import_source"), f"{label}.import_source"
        )
        for salvage_index, salvage in enumerate(
            _as_list(mapping.get("salvage", []), f"{label}.salvage")
        ):
            _as_mapping(salvage, f"{label}.salvage[{salvage_index}]")
        record_machine = _as_str(mapping["machine"], f"{label}.machine")
        if record_machine != selected:
            raise StateError(
                f"archived slot {slot} says machine {record_machine}, expected {selected}"
            )
        generation = _as_int(mapping["generation"], f"{label}.generation", minimum=1)
        _parse_timestamp(_as_str(mapping["created_at"], f"{label}.created_at"), label)
        finished_at = _as_str(mapping["finished_at"], f"{label}.finished_at")
        _parse_timestamp(finished_at, label)
        expected_archive_id = f"{selected}:{slot}:{generation}:{finished_at}"
        if archive_id != expected_archive_id:
            raise StateError(f"{label}.archive_id does not match its record")
        mode = _as_str(mapping["mode"], f"{label}.mode")
        if mode != "remove":
            raise StateError(f"{label}.mode is invalid: {mode!r}")
        actor = _as_str(mapping["actor"], f"{label}.actor")
        if actor != "coordinator":
            raise StateError(f"{label}.actor does not match its operation")
        if _as_str(mapping["physical_storage"], f"{label}.physical_storage") != "removed":
            raise StateError(f"{label} does not record removed physical storage")
        validation = _as_list(mapping["validation"], f"{label}.validation")
        if not validation:
            raise StateError(f"{label} has no validation evidence")
        for evidence_index, evidence in enumerate(validation):
            _as_str(evidence, f"{label}.validation[{evidence_index}]")
        for limitation_index, limitation in enumerate(
            _as_list(mapping["limitations"], f"{label}.limitations")
        ):
            _as_str(limitation, f"{label}.limitations[{limitation_index}]")
        if not _as_str(mapping["continuation"], f"{label}.continuation"):
            raise StateError(f"{label} has no continuation")
        checkouts = tuple(
            _checkout_from_obj(value, f"{label}.checkouts[{checkout_index}]")
            for checkout_index, value in enumerate(
                _as_list(mapping["checkouts"], f"{label}.checkouts")
            )
        )
        if len({checkout.name for checkout in checkouts}) != len(checkouts):
            raise StateError(f"{label} has duplicate checkout names")
        if not checkouts and (import_source is None or record_layout != "nested"):
            raise StateError(f"{label} has no checkouts")
        if record_layout == "flat" and len(checkouts) != 1:
            raise StateError(f"{label} has multiple checkouts under flat layout")
        if import_source is not None:
            source_row = import_source.row
            source_owner = _as_mapping(
                source_row.get("owner_sidecar"),
                f"{label}.import_source.row.owner_sidecar",
            )
            source_task, source_purpose = _historical_task_and_purpose(
                source_row, f"{label}.import_source.row", agent
            )
            if (
                record_layout != "nested"
                or source_task != mapping["task"]
                or source_purpose != mapping["purpose"]
                or source_row.get("allocated") != mapping["created_at"]
                or source_owner.get("slot") != slot
                or source_owner.get("agent") != agent
            ):
                raise StateError(f"{label} differs from its historical source row")
        for checkout in checkouts:
            expected, _destination = _checkout_path_for_layout(
                config, slot, checkout.name, slot_type, record_layout
            )
            if checkout.path != expected:
                raise StateError(
                    f"{label} checkout {checkout.name} path escaped its archived slot"
                )
            if checkout.landed_ref != _landed_ref_for_remote(config, checkout.remote):
                raise StateError(
                    f"{label} checkout {checkout.name} differs from configured authority"
                )
        records.append(dict(mapping))
    return ArchiveState(
        machine=actual_machine,
        revision=_as_int(raw["revision"], "archive state.revision"),
        records=tuple(records),
    )


def _load_archive_snapshot(config: Config, machine: str | None = None) -> ArchiveState:
    selected = machine or config.machine
    path = _archive_path(config, selected)
    return _archive_from_obj(
        config, _read_json(path, "archive state"), selected, str(path)
    )


def _ensure_state_shard(config: Config) -> None:
    active = _active_path(config)
    archive = _archive_path(config)
    derived = _states_from_events(config, config.machine)
    if derived is not None:
        active_state, archive_state = derived
        _atomic_write_json(active, _active_to_obj(active_state))
        _atomic_write_json(archive, _archive_to_obj(archive_state))
        return
    active_exists = active.exists() or active.is_symlink()
    archive_exists = archive.exists() or archive.is_symlink()
    if active_exists != archive_exists:
        if active_exists:
            active_state = _load_active(config)
            if active_state.revision != 0 or active_state.slots:
                raise StateError(
                    f"partial machine state for {config.machine}: non-empty ACTIVE has no ARCHIVED"
                )
            _atomic_write_json(
                archive, _archive_to_obj(ArchiveState(config.machine, 0, ()))
            )
        else:
            archive_state = _load_archive(config)
            if archive_state.revision != 0 or archive_state.records:
                raise StateError(
                    f"partial machine state for {config.machine}: non-empty ARCHIVED has no ACTIVE"
                )
            _atomic_write_json(
                active, _active_to_obj(ActiveState(config.machine, 0, ()))
            )
        _ensure_event_log(config)
        return
    if active_exists:
        _load_active(config)
        _load_archive(config)
        _ensure_event_log(config)
        return
    _atomic_write_json(active, _active_to_obj(ActiveState(config.machine, 0, ())))
    _atomic_write_json(archive, _archive_to_obj(ArchiveState(config.machine, 0, ())))
    _ensure_event_log(config)


def _archive_to_obj(state: ArchiveState) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "machine": state.machine,
        "revision": state.revision,
        "records": list(state.records),
    }


def _event_directory(config: Config, machine: str | None = None) -> Path:
    selected = machine or config.machine
    _validate_name(selected, "machine")
    return config.control / f"EVENTS.{selected}"


def _event_digest(value: Mapping[str, object]) -> str:
    encoded = json.dumps(
        dict(value), sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _event_files(directory: Path) -> list[Path]:
    if not directory.exists():
        return []
    if directory.is_symlink() or not directory.is_dir():
        raise StateError(f"append-only event log is not a real directory: {directory}")
    entries = tuple(directory.iterdir())
    complete_pattern = re.compile(r"[0-9]{20}\.json")
    partial_pattern = re.compile(r"[0-9]{20}\.json\.tmp\..+")
    unexpected = sorted(
        entry
        for entry in entries
        if complete_pattern.fullmatch(entry.name) is None
        and partial_pattern.fullmatch(entry.name) is None
    )
    if unexpected:
        raise StateError(
            f"append-only event log contains an unexpected entry: {unexpected[0]}; "
            "preserve it and run 'wrkslots doctor' before changing slot state"
        )
    return sorted(entry for entry in entries if complete_pattern.fullmatch(entry.name))


def _event_from_path(
    path: Path,
    machine: str,
    expected_sequence: int,
    previous: str,
    *,
    require_filename: bool = True,
) -> dict[str, object]:
    if require_filename and path.name != f"{expected_sequence:020d}.json":
        raise StateError(
            f"append-only event log has a sequence gap at {path}; "
            "preserve the log and run 'wrkslots doctor'"
        )
    raw = dict(_as_mapping(_read_json(path, "append-only event"), "append-only event"))
    _exact_keys(
        raw,
        {
            "schema",
            "machine",
            "sequence",
            "previous_sha256",
            "recorded_at",
            "kind",
            "payload",
            "sha256",
        },
        set(),
        "append-only event",
    )
    if _as_int(raw["schema"], "append-only event.schema") != EVENT_SCHEMA:
        raise StateError(f"unsupported append-only event schema in {path}")
    if _as_str(raw["machine"], "append-only event.machine") != machine:
        raise StateError(f"append-only event belongs to another machine: {path}")
    if _as_int(raw["sequence"], "append-only event.sequence") != expected_sequence:
        raise StateError(f"append-only event sequence does not match its filename: {path}")
    if _as_str(raw["previous_sha256"], "append-only event.previous_sha256") != previous:
        raise StateError(f"append-only event hash chain is broken at {path}")
    _parse_timestamp(
        _as_str(raw["recorded_at"], "append-only event.recorded_at"),
        "append-only event.recorded_at",
    )
    _as_str(raw["kind"], "append-only event.kind")
    _as_mapping(raw["payload"], "append-only event.payload")
    recorded_digest = _as_str(raw["sha256"], "append-only event.sha256")
    core = {key: value for key, value in raw.items() if key != "sha256"}
    if recorded_digest != _event_digest(core):
        raise StateError(f"append-only event digest does not match its content: {path}")
    return raw


def _load_event_paths(
    paths: Sequence[Path], machine: str
) -> tuple[dict[str, object], ...]:
    events: list[dict[str, object]] = []
    previous = "0" * 64
    for expected_sequence, path in enumerate(paths, start=1):
        raw = _event_from_path(path, machine, expected_sequence, previous)
        previous = _as_str(raw["sha256"], "append-only event.sha256")
        events.append(raw)
    return tuple(events)


def _load_events(config: Config, machine: str | None = None) -> tuple[dict[str, object], ...]:
    selected = machine or config.machine
    directory = _event_directory(config, selected)
    return _load_event_paths(_event_files(directory), selected)


def _write_event_file(
    config: Config, machine: str, kind: str, payload: Mapping[str, object]
) -> dict[str, object]:
    directory = _event_directory(config, machine)
    if directory.exists() and (directory.is_symlink() or not directory.is_dir()):
        raise StateError(f"append-only event log is not a real directory: {directory}")
    created = not directory.exists()
    directory.mkdir(mode=0o755, parents=True, exist_ok=True)
    if created:
        _fsync_directory(directory.parent)
    events = _load_events(config, machine)
    sequence = len(events) + 1
    previous = "0" * 64 if not events else _as_str(
        events[-1]["sha256"], "append-only event.sha256"
    )
    core: dict[str, object] = {
        "schema": EVENT_SCHEMA,
        "machine": machine,
        "sequence": sequence,
        "previous_sha256": previous,
        "recorded_at": _utc_now(),
        "kind": kind,
        "payload": dict(payload),
    }
    event = {**core, "sha256": _event_digest(core)}
    path = directory / f"{sequence:020d}.json"
    if path.exists() or path.is_symlink():
        raise StateError(f"append-only event already exists and will not be replaced: {path}")
    _atomic_write_json(path, event)
    return event


def _ensure_event_log(config: Config, machine: str | None = None) -> None:
    selected = machine or config.machine
    if _event_files(_event_directory(config, selected)):
        return
    active = _load_active_snapshot(config, selected)
    archive = _load_archive_snapshot(config, selected)
    holds = [
        hold
        for record in active.slots
        if (hold := _load_hold_snapshot(config, record.slot, selected)) is not None
    ]
    _write_event_file(
        config,
        selected,
        "state-imported",
        {
            "active": _active_to_obj(active),
            "archive": _archive_to_obj(archive),
            "holds": holds,
        },
    )


def _states_from_events(
    config: Config, machine: str
) -> tuple[ActiveState, ArchiveState] | None:
    events = _load_events(config, machine)
    if not events:
        return None
    active: ActiveState | None = None
    archive: ArchiveState | None = None
    for event in events:
        payload = _as_mapping(event["payload"], "append-only event.payload")
        kind = _as_str(event["kind"], "append-only event.kind")
        if kind == "state-imported":
            if active is not None or archive is not None:
                raise StateError("append-only event log imports state more than once")
            active = _active_from_obj(
                config,
                payload.get("active"),
                machine,
                "append-only imported active state",
            )
            archive = _archive_from_obj(
                config,
                payload.get("archive"),
                machine,
                "append-only imported archive state",
            )
        elif kind == "active-state-recorded":
            if active is None:
                raise StateError("append-only event records active state before its import")
            _exact_keys(
                payload,
                {
                    "action",
                    "slot",
                    "previous_revision",
                    "revision",
                    "previous_record_sha256",
                    "record",
                    "evidence",
                },
                set(),
                "append-only active-record event",
            )
            slot = _as_str(payload["slot"], "append-only active-record event.slot")
            _validate_name(slot, "slot")
            previous_revision = _as_int(
                payload["previous_revision"],
                "append-only active-record event.previous_revision",
                minimum=0,
            )
            revision = _as_int(
                payload["revision"],
                "append-only active-record event.revision",
                minimum=1,
            )
            if previous_revision != active.revision or revision != active.revision + 1:
                raise StateError(
                    "append-only active-record event revision does not continue "
                    f"the derived state for slot {slot}"
                )
            current = next((item for item in active.slots if item.slot == slot), None)
            previous_digest = (
                None if current is None else _event_digest(_record_to_obj(current))
            )
            if payload["previous_record_sha256"] != previous_digest:
                raise StateError(
                    "append-only active-record event does not match the prior record "
                    f"for slot {slot}"
                )
            raw_record = payload["record"]
            if raw_record is None:
                if current is None:
                    raise StateError(
                        f"append-only active-record event removes absent slot {slot}"
                    )
                slots = tuple(item for item in active.slots if item.slot != slot)
            else:
                record = _record_from_obj(
                    raw_record, "append-only active-record event.record"
                )
                if record.slot != slot or record.machine != machine:
                    raise StateError(
                        "append-only active-record event record identity does not "
                        f"match slot {slot} on machine {machine}"
                    )
                slots = (
                    (*active.slots, record)
                    if current is None
                    else tuple(record if item.slot == slot else item for item in active.slots)
                )
            active = _active_from_obj(
                config,
                {
                    "schema": SCHEMA,
                    "machine": machine,
                    "revision": revision,
                    "slots": [_record_to_obj(item) for item in slots],
                },
                machine,
                "append-only derived active state",
            )
        elif kind == "archive-state-recorded":
            if archive is None:
                raise StateError("append-only event records archive state before its import")
            _exact_keys(
                payload,
                {
                    "action",
                    "slot",
                    "previous_revision",
                    "revision",
                    "record",
                    "evidence",
                },
                set(),
                "append-only archive-record event",
            )
            previous_revision = _as_int(
                payload["previous_revision"],
                "append-only archive-record event.previous_revision",
                minimum=0,
            )
            revision = _as_int(
                payload["revision"],
                "append-only archive-record event.revision",
                minimum=1,
            )
            if previous_revision != archive.revision or revision != archive.revision + 1:
                raise StateError(
                    "append-only archive-record event revision does not continue "
                    "the derived archive"
                )
            archive_record = dict(
                _as_mapping(payload["record"], "append-only archive-record event.record")
            )
            slot = _as_str(payload["slot"], "append-only archive-record event.slot")
            if archive_record.get("slot") != slot:
                raise StateError(
                    "append-only archive-record event record identity does not "
                    f"match slot {slot}"
                )
            archive = _archive_from_obj(
                config,
                {
                    "schema": SCHEMA,
                    "machine": machine,
                    "revision": revision,
                    "records": [*archive.records, archive_record],
                },
                machine,
                "append-only derived archive state",
            )
    if active is None or archive is None:
        raise StateError("append-only event log has no complete imported state")
    return active, archive


def _load_active(config: Config, machine: str | None = None) -> ActiveState:
    selected = machine or config.machine
    states = _states_from_events(config, selected)
    return _load_active_snapshot(config, selected) if states is None else states[0]


def _load_archive(config: Config, machine: str | None = None) -> ArchiveState:
    selected = machine or config.machine
    states = _states_from_events(config, selected)
    return _load_archive_snapshot(config, selected) if states is None else states[1]


def _write_active_state(
    config: Config,
    state: ActiveState,
    *,
    action: str,
    slot: str,
    evidence: Mapping[str, object] | None = None,
) -> None:
    _ensure_event_log(config, state.machine)
    previous = _load_active(config, state.machine)
    if state.revision != previous.revision + 1:
        raise StateError(
            "active state revision must advance exactly once before it is recorded"
        )
    previous_by_slot = {item.slot: item for item in previous.slots}
    current_by_slot = {item.slot: item for item in state.slots}
    changed = {
        name
        for name in previous_by_slot.keys() | current_by_slot.keys()
        if previous_by_slot.get(name) != current_by_slot.get(name)
    }
    previous_record = previous_by_slot.get(slot)
    current_record = current_by_slot.get(slot)
    if changed not in ({slot}, set()) or (
        not changed and (previous_record is None or current_record is None)
    ):
        raise StateError(
            "active state mutation must affect only its named slot; "
            f"expected {slot!r}, changed {sorted(changed)!r}"
        )
    _write_event_file(
        config,
        state.machine,
        "active-state-recorded",
        {
            "action": action,
            "slot": slot,
            "previous_revision": previous.revision,
            "revision": state.revision,
            "previous_record_sha256": (
                None
                if previous_record is None
                else _event_digest(_record_to_obj(previous_record))
            ),
            "record": (
                None if current_record is None else _record_to_obj(current_record)
            ),
            "evidence": dict(evidence or {}),
        },
    )
    _atomic_write_json(_active_path(config, state.machine), _active_to_obj(state))


def _write_archive_state(
    config: Config,
    state: ArchiveState,
    *,
    action: str,
    slot: str,
    evidence: Mapping[str, object] | None = None,
) -> None:
    _ensure_event_log(config, state.machine)
    previous = _load_archive(config, state.machine)
    if state.revision != previous.revision + 1:
        raise StateError(
            "archive state revision must advance exactly once before it is recorded"
        )
    if (
        len(state.records) != len(previous.records) + 1
        or tuple(state.records[:-1]) != previous.records
    ):
        raise StateError("archive mutation must append exactly one record")
    record = dict(state.records[-1])
    if record.get("slot") != slot:
        raise StateError("archive mutation record does not match its named slot")
    _write_event_file(
        config,
        state.machine,
        "archive-state-recorded",
        {
            "action": action,
            "slot": slot,
            "previous_revision": previous.revision,
            "revision": state.revision,
            "record": record,
            "evidence": dict(evidence or {}),
        },
    )
    _atomic_write_json(_archive_path(config, state.machine), _archive_to_obj(state))


def _write_journal(config: Config, payload: Mapping[str, object]) -> None:
    _ensure_event_log(config)
    _write_event_file(
        config,
        config.machine,
        "operation-progress-recorded",
        {
            "slot": payload.get("slot"),
            "operation": payload.get("kind"),
            "journal": dict(payload),
        },
    )
    _atomic_write_json(_journal_path(config), dict(payload))


def _clear_journal(config: Config, payload: Mapping[str, object]) -> None:
    _ensure_event_log(config)
    _write_event_file(
        config,
        config.machine,
        "operation-completed",
        {"slot": payload.get("slot"), "operation": payload.get("kind")},
    )
    path = _journal_path(config)
    if path.exists() or path.is_symlink():
        _remove_control_file(path)


def _pending_operation_from_events(
    config: Config, machine: str
) -> Mapping[str, object] | None:
    pending: Mapping[str, object] | None = None
    for event in _load_events(config, machine):
        kind = _as_str(event["kind"], "append-only event.kind")
        payload = _as_mapping(event["payload"], "append-only event.payload")
        if kind == "operation-progress-recorded":
            pending = _as_mapping(payload.get("journal"), "append-only operation journal")
        elif kind == "operation-completed" and pending is not None:
            if (
                payload.get("slot") == pending.get("slot")
                and payload.get("operation") == pending.get("kind")
            ):
                pending = None
    return pending


def _recorded_handoff_digest(config: Config, record: ActiveRecord) -> str | None:
    digest: str | None = None
    for event in _load_events(config, record.machine):
        if event.get("kind") != "handoff-read":
            continue
        payload = _as_mapping(event["payload"], "handoff-read event payload")
        if payload.get("slot") == record.slot and payload.get("generation") == record.generation:
            digest = _as_str(payload.get("sha256"), "handoff-read event sha256")
    return digest


def _empty_state_repair_payload(
    path: Path, label: str, machine: str, rows_field: str
) -> dict[str, object] | None:
    raw = _as_mapping(_read_json(path, label), label)
    _exact_keys(raw, {"schema", "machine", "revision", rows_field}, set(), label)
    schema = _as_int(raw["schema"], f"{label}.schema")
    if schema == SCHEMA:
        return None
    if schema != 1:
        raise StateError(f"unsupported {label} schema in {path}")
    actual_machine = _as_str(raw["machine"], f"{label}.machine")
    if actual_machine != machine:
        raise StateError(
            f"{label} machine mismatch: filename says {machine}, content says "
            f"{actual_machine}"
        )
    revision = _as_int(raw["revision"], f"{label}.revision")
    rows = _as_list(raw[rows_field], f"{label}.{rows_field}")
    if revision != 0 or rows:
        raise StateError(
            f"cannot repair non-empty {label} written with schema 1 in {path}"
        )
    return {
        "schema": SCHEMA,
        "machine": machine,
        "revision": 0,
        rows_field: [],
    }


def _repair_empty_state_from_schema_one(config: Config) -> None:
    active_path = _active_path(config)
    archive_path = _archive_path(config)
    active_payload = (
        _empty_state_repair_payload(
            active_path, "active state", config.machine, "slots"
        )
        if active_path.exists() or active_path.is_symlink()
        else None
    )
    archive_payload = (
        _empty_state_repair_payload(
            archive_path, "archive state", config.machine, "records"
        )
        if archive_path.exists() or archive_path.is_symlink()
        else None
    )

    # Validate both durable files before rewriting either one. A malformed
    # current file must not leave the pair half-repaired.
    if active_payload is None and (active_path.exists() or active_path.is_symlink()):
        _load_active(config)
    if archive_payload is None and (
        archive_path.exists() or archive_path.is_symlink()
    ):
        _load_archive(config)

    for path, payload in (
        (active_path, active_payload),
        (archive_path, archive_payload),
    ):
        if payload is None:
            continue
        _atomic_write_json(path, payload)
        print(f"REPAIRED {path}: schema 1 -> {SCHEMA}")


def _atomic_replace_symlink(path: Path, target: Path) -> None:
    temporary = path.parent / f".{path.name}.tmp.{os.getpid()}.{uuid.uuid4().hex}"
    try:
        temporary.symlink_to(target)
        os.replace(temporary, path)
        _fsync_directory(path.parent)
    finally:
        if temporary.is_symlink():
            temporary.unlink()


def _state_files(config: Config) -> list[tuple[str, Path]]:
    result: list[tuple[str, Path]] = []
    pattern = re.compile(r"^ACTIVE\.(?P<machine>[A-Za-z0-9][A-Za-z0-9._-]{0,63})\.json$")
    for path in sorted(config.control.glob("ACTIVE.*.json")):
        match = pattern.fullmatch(path.name)
        if match is None:
            raise StateError(f"unexpected active-state filename: {path}")
        result.append((match.group("machine"), path))
    return result


def _archive_files(config: Config) -> list[tuple[str, Path]]:
    result: list[tuple[str, Path]] = []
    pattern = re.compile(
        r"^ARCHIVED\.(?P<machine>[A-Za-z0-9][A-Za-z0-9._-]{0,63})\.json$"
    )
    for path in sorted(config.control.glob("ARCHIVED.*.json")):
        match = pattern.fullmatch(path.name)
        if match is None:
            raise StateError(f"unexpected archive-state filename: {path}")
        result.append((match.group("machine"), path))
    return result


def _state_machines(config: Config) -> tuple[str, ...]:
    active = {machine for machine, _path in _state_files(config)}
    archive = {machine for machine, _path in _archive_files(config)}
    events: set[str] = set()
    pattern = re.compile(r"^EVENTS\.(?P<machine>[A-Za-z0-9][A-Za-z0-9._-]{0,63})$")
    for path in sorted(config.control.glob("EVENTS.*")):
        match = pattern.fullmatch(path.name)
        if match is None:
            raise StateError(f"unexpected append-only event directory name: {path}")
        if path.is_symlink() or not path.is_dir():
            raise StateError(f"append-only event log is not a real directory: {path}")
        events.add(match.group("machine"))
    missing_pairs = (active ^ archive) - events
    if missing_pairs:
        raise StateError(
            "ACTIVE and ARCHIVED machine shards differ without append-only history: "
            f"active={sorted(active)} archived={sorted(archive)} events={sorted(events)}"
        )
    return tuple(sorted(active | archive | events))


def _load_all_active(config: Config) -> list[ActiveState]:
    states = [_load_active(config, machine) for machine in _state_machines(config)]
    global_slots: dict[str, str] = {}
    global_agents: dict[str, str] = {}
    for state in states:
        for record in state.slots:
            previous_slot = global_slots.setdefault(record.slot, state.machine)
            if previous_slot != state.machine:
                raise StateError(
                    f"slot {record.slot!r} is active in both {previous_slot} and {state.machine}"
                )
            if record.slot_type == "agent" and record.import_source is None:
                previous_agent = global_agents.setdefault(record.agent, record.slot)
                if previous_agent != record.slot:
                    raise StateError(
                        f"agent {record.agent!r} owns both {previous_agent} and {record.slot}"
                    )
    return states


def _load_all_archives(config: Config) -> list[ArchiveState]:
    archives = [_load_archive(config, machine) for machine in _state_machines(config)]
    slots: dict[str, str] = {}
    for archive in archives:
        for record in archive.records:
            slot = _as_str(record.get("slot"), "archive slot")
            previous = slots.setdefault(slot, archive.machine)
            if previous != archive.machine:
                raise StateError(
                    f"slot {slot!r} is archived in both {previous} and {archive.machine}"
                )
    return archives


def _validate_global_state(
    config: Config,
) -> tuple[list[ActiveState], list[ArchiveState]]:
    states = _load_all_active(config)
    archives = _load_all_archives(config)
    active_slots = {
        record.slot: state.machine for state in states for record in state.slots
    }
    for archive in archives:
        for record in archive.records:
            slot = _as_str(record.get("slot"), "archive slot")
            if slot in active_slots:
                raise StateError(
                    f"slot {slot!r} is active on {active_slots[slot]} and archived on {archive.machine}"
                )
    return states, archives


def _validate_global_state_for_finish_recovery(
    config: Config, journal: Mapping[str, object]
) -> tuple[list[ActiveState], list[ArchiveState]]:
    states = _load_all_active(config)
    archives = _load_all_archives(config)
    recorded = _record_from_obj(journal.get("record"), "finish journal.record")
    expected_entry = _archive_entry(journal, recorded)
    active = {
        record.slot: (state.machine, record)
        for state in states
        for record in state.slots
    }
    for archive in archives:
        for archived in archive.records:
            slot = _as_str(archived.get("slot"), "archive slot")
            current = active.get(slot)
            if current is None:
                continue
            active_machine, active_record = current
            if (
                slot == recorded.slot
                and active_machine == config.machine
                and archive.machine == config.machine
                and _record_to_obj(active_record) == _record_to_obj(recorded)
                and _json_equal(archived, expected_entry)
            ):
                continue
            raise StateError(
                f"slot {slot!r} is active on {active_machine} and archived on {archive.machine}"
            )
    return states, archives


def _assert_registry_storage_consistent(
    config: Config,
    states: Sequence[ActiveState],
    allowed_unregistered_slot: str | None = None,
    *,
    allow_unregistered_migration_slots: bool = False,
) -> tuple[str, ...]:
    records = [record for state in states for record in state.slots]
    expected_slots = {
        slot_type: {record.slot for record in records if record.slot_type == slot_type}
        for slot_type in SLOT_TYPES
    }
    for record in records:
        _assert_slot_contents(config, record)
        vcs = _GitVcs()
        for checkout in record.checkouts:
            path = _stored_path(config, checkout.path, "checkout path")
            _relative, repository = _stored_repository_path(config, checkout.repository)
            vcs.verify_existing_worktree(repository, path)
    unexpected: list[str] = []
    for slot_type, root in _slot_roots(config).items():
        if not root.exists():
            continue
        if root.is_symlink() or not root.is_dir():
            raise StateError(f"managed {slot_type} slots directory is unsafe: {root}")
        actual_slots = {
            entry.name
            for entry in root.iterdir()
            if entry.is_dir()
            and not entry.is_symlink()
            and not entry.name.startswith("EVENTS.")
            and not (
                slot_type == "agent"
                and entry == _validate_slots_directory(config)
            )
        }
        unexpected.extend(
            f"{slot_type}:{slot}" for slot in sorted(actual_slots - expected_slots[slot_type])
        )
    if allowed_unregistered_slot is not None:
        unexpected = [
            value
            for value in unexpected
            if value.split(":", 1)[1] != allowed_unregistered_slot
        ]
    if unexpected and not allow_unregistered_migration_slots:
        slot_type, slot = unexpected[0].split(":", 1)
        if slot_type == "agent":
            remedy = (
                "run 'wrkslots audit --format json', then "
                f"'wrkslots register {slot} --help' and register the live checkout "
                "with --agent, --task, --purpose, --owner-pid, --coordinator-pid, "
                "--verified-live, and --repo. Put --allow-existing-unregistered-worktrees "
                "before unrelated commands only during deliberate migration; do not delete "
                "the directory to repair bookkeeping"
            )
        else:
            remedy = (
                "run 'wrkslots audit --format json', then import the retained validation "
                "state with 'wrkslots import-existing --help'"
            )
        raise StateError(
            "managed worktrees directory has a directory without an active row: "
            f"{unexpected[0]}. state: REFUSED -- no registered slot or unregistered "
            "directory was changed",
            remedy=remedy,
        )
    return tuple(unexpected)


def _assert_command_registry_storage(
    config: Config,
    states: Sequence[ActiveState],
    args: argparse.Namespace,
    *,
    allowed_unregistered_slot: str | None = None,
    migration_operation: bool = False,
) -> None:
    allow = bool(
        migration_operation
        or getattr(args, "allow_existing_unregistered_worktrees", False)
    )
    unexpected = _assert_registry_storage_consistent(
        config,
        states,
        allowed_unregistered_slot=allowed_unregistered_slot,
        allow_unregistered_migration_slots=allow,
    )
    if unexpected and allow:
        print(
            f"WARNING: {len(unexpected)} existing unregistered worktree directories "
            "remain outside wrkslots records. state: RETAINED -- this command will not "
            "inspect, select, or remove them. remedy: run 'wrkslots audit --format json' "
            "and import each live slot from verified process evidence",
            file=sys.stderr,
        )


def _global_rows(
    states: Sequence[ActiveState], archives: Sequence[ArchiveState]
) -> dict[tuple[str, str, str], object]:
    rows: dict[tuple[str, str, str], object] = {}
    for state in states:
        for record in state.slots:
            rows[("active", state.machine, record.slot)] = _record_to_obj(record)
    for archive in archives:
        for archive_record in archive.records:
            slot = _as_str(archive_record.get("slot"), "archive slot")
            rows[("archive", archive.machine, slot)] = dict(archive_record)
    return rows


def _assert_only_slot_changed(
    before: Mapping[tuple[str, str, str], object],
    after: Mapping[tuple[str, str, str], object],
    slot: str,
) -> None:
    before_unrelated = {key: value for key, value in before.items() if key[2] != slot}
    after_unrelated = {key: value for key, value in after.items() if key[2] != slot}
    if before_unrelated != after_unrelated:
        raise StateError(f"mutation of slot {slot!r} changed an unrelated registry row")


def _find_record(state: ActiveState, slot: str) -> ActiveRecord:
    for record in state.slots:
        if record.slot == slot:
            return record
    raise Refusal(f"slot {slot!r} is not active on machine {state.machine}")


def _replace_record(state: ActiveState, record: ActiveRecord) -> ActiveState:
    found = False
    slots: list[ActiveRecord] = []
    for current in state.slots:
        if current.slot == record.slot:
            slots.append(record)
            found = True
        else:
            slots.append(current)
    if not found:
        raise StateError(f"cannot replace missing slot {record.slot}")
    return ActiveState(state.machine, state.revision + 1, tuple(slots))


def _delete_record(state: ActiveState, slot: str) -> ActiveState:
    slots = tuple(record for record in state.slots if record.slot != slot)
    if len(slots) == len(state.slots):
        raise StateError(f"cannot remove missing slot {slot}")
    return ActiveState(state.machine, state.revision + 1, slots)


def _append_record(state: ActiveState, record: ActiveRecord) -> ActiveState:
    if any(item.slot == record.slot for item in state.slots):
        raise Refusal(f"slot {record.slot!r} is already active")
    if record.slot_type == "agent" and record.import_source is None and any(
        item.slot_type == "agent"
        and item.import_source is None
        and item.agent == record.agent
        for item in state.slots
    ):
        raise Refusal(f"agent {record.agent!r} already owns an active slot")
    return ActiveState(state.machine, state.revision + 1, (*state.slots, record))


class _GitVcs:
    """The small Git boundary used by slot operations."""

    @staticmethod
    def _run(
        repository: Path,
        args: Sequence[str],
        *,
        check: bool = True,
        input_text: str | None = None,
        env_overrides: Mapping[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("GIT_")
        }
        env["LC_ALL"] = "C"
        env["GIT_CONFIG_NOSYSTEM"] = "1"
        env["GIT_CONFIG_GLOBAL"] = "/dev/null"
        env["GIT_CONFIG_SYSTEM"] = "/dev/null"
        env["GIT_NO_REPLACE_OBJECTS"] = "1"
        if env_overrides:
            env.update(env_overrides)
        try:
            completed = subprocess.run(
                [
                    "git",
                    "--no-replace-objects",
                    "-c",
                    "core.useReplaceRefs=false",
                    "-C",
                    str(repository),
                    *args,
                ],
                text=True,
                capture_output=True,
                check=False,
                env=env,
                input=input_text,
            )
        except OSError as exc:
            raise Refusal(f"cannot execute Git: {exc}") from exc
        if check and completed.returncode != 0:
            detail = (completed.stderr or completed.stdout).strip()
            raise Refusal(
                f"Git refused in {repository}: git {' '.join(args)}"
                + (f": {detail}" if detail else "")
            )
        return completed

    def repository_root(self, repository: Path) -> Path:
        result = self._run(repository, ["rev-parse", "--show-toplevel"])
        root = Path(result.stdout.strip()).absolute()
        if root.is_symlink() or not root.is_dir():
            raise Refusal(f"Git reported an unsafe repository root: {root}")
        return root

    def common_directory(self, repository: Path) -> Path:
        result = self._run(repository, ["rev-parse", "--path-format=absolute", "--git-common-dir"])
        common = Path(result.stdout.strip()).absolute()
        if common.is_symlink() or not common.is_dir():
            raise Refusal(f"Git reported an unsafe common directory: {common}")
        return common

    def verify_ref(self, repository: Path, ref: str, label: str) -> str:
        _validate_ref(ref, label)
        result = self._run(repository, ["rev-parse", "--verify", f"{ref}^{{commit}}"])
        sha = result.stdout.strip()
        if not SHA_RE.fullmatch(sha):
            raise Refusal(f"{label} did not resolve to one full commit: {ref}")
        return sha

    def check_branch_name(self, repository: Path, branch: str) -> None:
        _validate_ref(branch, "branch")
        result = self._run(repository, ["check-ref-format", "--branch", branch], check=False)
        if result.returncode != 0:
            raise Refusal(f"invalid Git branch name {branch!r}")

    def branch_exists(self, repository: Path, branch: str) -> bool:
        result = self._run(
            repository, ["show-ref", "--verify", "--quiet", f"refs/heads/{branch}"], check=False
        )
        if result.returncode not in (0, 1):
            raise Refusal(f"cannot determine whether branch {branch!r} exists in {repository}")
        return result.returncode == 0

    def add_worktree(
        self, repository: Path, destination: Path, branch: str, start_point: str
    ) -> str:
        self.check_branch_name(repository, branch)
        if self.branch_exists(repository, branch):
            raise Refusal(f"local branch already exists: {branch}")
        self.verify_ref(repository, start_point, "start point")
        self._run(
            repository,
            ["worktree", "add", "-b", branch, str(destination), start_point],
        )
        return self.head(destination)

    def listed_worktrees(self, repository: Path) -> set[Path]:
        result = self._run(repository, ["worktree", "list", "--porcelain"])
        paths: set[Path] = set()
        for line in result.stdout.splitlines():
            if line.startswith("worktree "):
                paths.add(Path(line.removeprefix("worktree ")).absolute())
        return paths

    def verify_existing_worktree(self, repository: Path, checkout: Path) -> str:
        source_common = self.common_directory(repository)
        checkout_root = self.repository_root(checkout)
        if checkout_root != checkout.absolute():
            raise Refusal(
                f"checkout path is not its Git worktree root: {checkout} (root {checkout_root})"
            )
        if self.common_directory(checkout) != source_common:
            raise Refusal(f"checkout {checkout} does not belong to repository {repository}")
        if checkout.absolute() not in self.listed_worktrees(repository):
            raise Refusal(f"Git does not list {checkout} as a worktree of {repository}")
        return self.head(checkout)

    def head(self, checkout: Path) -> str:
        result = self._run(checkout, ["rev-parse", "--verify", "HEAD^{commit}"])
        sha = result.stdout.strip()
        if not SHA_RE.fullmatch(sha):
            raise Refusal(f"checkout HEAD is not a full commit: {checkout}")
        return sha

    def branch(self, checkout: Path) -> str:
        result = self._run(checkout, ["symbolic-ref", "--quiet", "--short", "HEAD"], check=False)
        if result.returncode != 0:
            raise Refusal(f"checkout must be on its registered branch, not detached: {checkout}")
        branch = result.stdout.strip()
        _validate_ref(branch, "branch")
        return branch

    def status(self, checkout: Path, cache_globs: Sequence[str] = ()) -> str:
        pathspecs = ["."]
        for pattern in cache_globs:
            pathspecs.append(f":(exclude,glob){pattern}")
            pathspecs.append(f":(exclude,glob){pattern}/**")
        result = self._run(
            checkout,
            [
                "status",
                "--porcelain=v2",
                "--untracked-files=all",
                "--ignored=matching",
                "--",
                *pathspecs,
            ],
        )
        return result.stdout

    def tracked_cache_paths(
        self, checkout: Path, cache_globs: Sequence[str]
    ) -> tuple[str, ...]:
        if not cache_globs:
            return ()
        indexed = self._run(checkout, ["ls-files", "--recurse-submodules", "-z"])
        committed = self._run(
            checkout,
            ["ls-tree", "-r", "--name-only", "-z", "HEAD"],
        )
        paths = [
            path
            for output in (indexed.stdout, committed.stdout)
            for path in output.split("\x00")
            if path
            and any(
                _cache_glob_contains_path(pattern, path) for pattern in cache_globs
            )
        ]
        return tuple(dict.fromkeys(paths))

    def submodule_paths(self, checkout: Path) -> tuple[str, ...]:
        indexed = self._run(checkout, ["ls-files", "--stage", "-z"])
        committed = self._run(checkout, ["ls-tree", "-r", "-z", "HEAD"])
        paths: list[str] = []
        for output in (indexed.stdout, committed.stdout):
            for record in output.split("\x00"):
                if not record:
                    continue
                metadata, separator, path = record.partition("\t")
                if separator and metadata.startswith("160000 "):
                    paths.append(path)
        return tuple(dict.fromkeys(paths))

    def initialized_submodules(self, checkout: Path) -> tuple[str, ...]:
        result = self._run(
            checkout,
            [
                "submodule",
                "foreach",
                "--quiet",
                "--recursive",
                'printf "%s\\n" "$displaypath"',
            ],
        )
        paths = tuple(line for line in result.stdout.splitlines() if line)
        for path in paths:
            candidate = Path(path)
            if candidate.is_absolute() or ".." in candidate.parts:
                raise Refusal(
                    f"Git reported an unsafe initialized submodule path: {path!r}"
                )
        return tuple(dict.fromkeys(paths))

    def assert_ordinary_index(self, checkout: Path) -> None:
        assume = self._run(checkout, ["ls-files", "-v"])
        for line in assume.stdout.splitlines():
            if line and line[0].islower():
                raise Refusal(
                    f"checkout has assume-unchanged index state: {line[2:]}"
                )
        skip = self._run(checkout, ["ls-files", "-t"])
        for line in skip.stdout.splitlines():
            if line.startswith("S "):
                raise Refusal(f"checkout has skip-worktree index state: {line[2:]}")

    def assert_ordinary_history(self, checkout: Path) -> None:
        common = self.common_directory(checkout)
        grafts = common / "info" / "grafts"
        shallow = common / "shallow"
        if grafts.exists() or grafts.is_symlink():
            raise Refusal(f"checkout uses graft history: {grafts}")
        if shallow.exists() or shallow.is_symlink():
            raise Refusal(f"checkout uses shallow history: {shallow}")
        replacements = self._run(
            checkout,
            ["for-each-ref", "--format=%(refname)", "refs/replace/"],
        )
        if replacements.stdout.strip():
            raise Refusal(
                f"checkout contains replacement refs: {replacements.stdout.splitlines()[0]}"
            )

    def fetch_remote(self, checkout: Path, remote: str, landed_ref: str) -> None:
        _validate_remote(remote)
        _validate_full_ref(landed_ref, "landed ref")
        prefix = f"refs/remotes/{remote}/"
        if not landed_ref.startswith(prefix):
            raise Refusal(f"landed ref must be under {prefix}: {landed_ref}")
        self._run(
            checkout,
            [
                "fetch",
                "--prune",
                "--no-tags",
                "--no-recurse-submodules",
                remote,
                f"+refs/heads/*:{prefix}*",
            ],
        )

    def remote_url_sha256(self, checkout: Path, remote: str) -> str:
        _validate_remote(remote)
        result = self._run(checkout, ["remote", "get-url", "--all", remote])
        urls = result.stdout.splitlines()
        if len(urls) != 1 or not urls[0]:
            raise Refusal(
                f"remote {remote!r} must have exactly one non-empty fetch URL"
            )
        return hashlib.sha256(urls[0].encode("utf-8")).hexdigest()

    def assert_remote_url(self, checkout: Path, remote: str, authorized_url: str) -> str:
        _validate_remote(remote)
        result = self._run(checkout, ["remote", "get-url", "--all", remote])
        urls = result.stdout.splitlines()
        if urls != [authorized_url]:
            raise Refusal(
                f"remote {remote!r} URL differs from trusted provisioning input"
            )
        return hashlib.sha256(authorized_url.encode("utf-8")).hexdigest()

    def operation_paths(self, checkout: Path) -> list[Path]:
        names = (
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "rebase-apply",
            "rebase-merge",
            "sequencer",
        )
        found: list[Path] = []
        for name in names:
            result = self._run(checkout, ["rev-parse", "--git-path", name])
            candidate = Path(result.stdout.strip())
            if not candidate.is_absolute():
                candidate = checkout / candidate
            if candidate.exists() or candidate.is_symlink():
                found.append(candidate)
        return found

    def remote_refs_containing(self, checkout: Path, remote: str, head: str) -> tuple[str, ...]:
        _validate_remote(remote)
        result = self._run(
            checkout,
            [
                "for-each-ref",
                "--format=%(refname)",
                "--contains",
                head,
                f"refs/remotes/{remote}/",
            ],
        )
        refs = tuple(line for line in result.stdout.splitlines() if line)
        return refs

    def ref_exists(self, checkout: Path, ref: str) -> bool:
        _validate_full_ref(ref, "remote ref")
        result = self._run(
            checkout, ["show-ref", "--verify", "--quiet", ref], check=False
        )
        if result.returncode not in (0, 1):
            raise Refusal(f"cannot determine whether ref {ref} exists")
        return result.returncode == 0

    def commit_paths(self, checkout: Path, commit: str) -> tuple[str, ...]:
        if not SHA_RE.fullmatch(commit):
            raise Refusal(f"cannot inspect paths for invalid commit {commit!r}")
        result = self._run(
            checkout,
            [
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-m",
                "-z",
                commit,
            ],
        )
        return tuple(dict.fromkeys(path for path in result.stdout.split("\x00") if path))

    def is_ancestor(self, checkout: Path, ancestor: str, descendant: str) -> bool:
        self.verify_ref(checkout, descendant, "landed ref")
        result = self._run(
            checkout,
            ["merge-base", "--is-ancestor", ancestor, descendant],
            check=False,
        )
        if result.returncode not in (0, 1):
            raise Refusal(
                f"cannot compare checkout HEAD {ancestor} with landed ref {descendant}"
            )
        return result.returncode == 0

    def remove_worktree(
        self, repository: Path, checkout: Path, *, force: bool = False
    ) -> None:
        args = ["worktree", "remove"]
        if force:
            args.append("--force")
        args.extend(("--", str(checkout)))
        self._run(repository, args)

    def remote_ref_sha(self, checkout: Path, remote: str, ref: str) -> str | None:
        _validate_remote(remote)
        _validate_full_ref(ref, "remote salvage ref")
        result = self._run(
            checkout, ["ls-remote", "--refs", remote, ref], check=False
        )
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip()
            raise Refusal(
                f"cannot read remote {remote!r} salvage ref {ref}"
                + (f": {detail}" if detail else "")
            )
        lines = [line.split() for line in result.stdout.splitlines() if line.strip()]
        if not lines:
            return None
        if len(lines) != 1 or len(lines[0]) != 2 or lines[0][1] != ref:
            raise Refusal(f"remote returned ambiguous content for salvage ref {ref}")
        sha = lines[0][0]
        if not SHA_RE.fullmatch(sha):
            raise Refusal(f"remote salvage ref {ref} did not resolve to one full commit")
        return sha

    def push_salvage(
        self, checkout: Path, remote: str, commit: str, ref: str
    ) -> None:
        existing = self.remote_ref_sha(checkout, remote, ref)
        if existing is not None:
            if existing != commit:
                raise Refusal(
                    f"remote salvage ref {ref} already names {existing}, not {commit}; "
                    "preserve the checkout and choose a different recorded destination"
                )
            return
        self._run(checkout, ["push", remote, f"{commit}:{ref}"])
        observed = self.remote_ref_sha(checkout, remote, ref)
        if observed != commit:
            raise Refusal(
                f"salvage push did not leave {ref} at {commit}; remote reports "
                f"{observed or 'no ref'}. Preserve the checkout and retry"
            )

    def delete_branch_at(self, repository: Path, branch: str, expected_head: str) -> None:
        if not self.branch_exists(repository, branch):
            return
        actual = self.verify_ref(repository, f"refs/heads/{branch}", "created branch")
        if actual != expected_head:
            raise Refusal(
                f"created branch {branch} moved from {expected_head} to {actual}; "
                "preserve it instead of aborting provisioning"
            )
        self._run(repository, ["branch", "-D", "--", branch])

    def repair_worktree(self, repository: Path, checkout: Path) -> None:
        self._run(repository, ["worktree", "repair", str(checkout)])
        if checkout.absolute() not in self.listed_worktrees(repository):
            raise Refusal(f"Git did not repair worktree registration for {checkout}")

    def repair_nested_repository(self, checkout: Path) -> None:
        git_file = checkout / ".git"
        if git_file.is_symlink() or not git_file.is_file():
            raise Refusal(f"nested repository has no safe .git file: {git_file}")
        try:
            value = git_file.read_text(encoding="utf-8").strip()
        except (OSError, UnicodeError) as exc:
            raise Refusal(f"cannot read nested repository identity {git_file}: {exc}") from exc
        prefix = "gitdir: "
        if not value.startswith(prefix) or "\n" in value:
            raise Refusal(f"nested repository has an invalid .git file: {git_file}")
        raw = Path(value.removeprefix(prefix))
        admin = raw if raw.is_absolute() else checkout / raw
        try:
            admin = admin.resolve(strict=True)
        except OSError as exc:
            raise Refusal(f"nested repository Git directory is unavailable: {admin}: {exc}") from exc
        if admin.is_symlink() or not admin.is_dir():
            raise Refusal(f"nested repository Git directory is unsafe: {admin}")
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("GIT_")
        }
        environment.update(
            {
                "LC_ALL": "C",
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": "/dev/null",
                "GIT_CONFIG_SYSTEM": "/dev/null",
                "GIT_NO_REPLACE_OBJECTS": "1",
            }
        )
        try:
            completed = subprocess.run(
                [
                    "git",
                    "--no-replace-objects",
                    "-c",
                    "core.useReplaceRefs=false",
                    f"--git-dir={admin}",
                    f"--work-tree={checkout}",
                    "config",
                    "core.worktree",
                    str(checkout),
                ],
                text=True,
                capture_output=True,
                check=False,
                env=environment,
            )
        except OSError as exc:
            raise Refusal(f"cannot execute Git while repairing {checkout}: {exc}") from exc
        if completed.returncode != 0:
            detail = (completed.stderr or completed.stdout).strip()
            raise Refusal(
                f"Git refused nested repository repair for {checkout}"
                + (f": {detail}" if detail else "")
            )
        if self.repository_root(checkout) != checkout.absolute():
            raise Refusal(f"Git did not repair nested repository worktree {checkout}")


def _resolved_repository_path(
    config: Config,
    stored: str,
    unresolved: Path,
    allowed_root: Path,
) -> tuple[str, Path]:
    _ensure_no_symlink_components(allowed_root, unresolved, "repository")
    try:
        absolute = unresolved.resolve(strict=True)
    except OSError as exc:
        raise Refusal(
            f"repository does not exist or cannot be resolved: {unresolved}",
            remedy="pass --repo NAME=PATH with an existing Git worktree path",
        ) from exc
    if not absolute.is_dir():
        raise Refusal(f"repository does not exist or is unsafe: {absolute}")
    if _path_is_within(absolute, _managed_worktrees_root(config)):
        raise Refusal(
            f"source repository must be outside the managed worktrees directory: {absolute}"
        )
    return stored, absolute


def _repository_path(config: Config, raw: str) -> tuple[str, Path]:
    candidate = Path(raw)
    if candidate.is_absolute():
        raise Refusal(
            f"repository path must be relative to {config.root}: {raw!r}; "
            "use a path inside the project root or ../NAME components for one direct sibling"
        )
    parts = candidate.parts
    is_direct_sibling = (
        len(parts) == 2 and parts[0] == ".." and parts[1] not in {".", ".."}
    )
    if ".." in parts and not is_direct_sibling:
        raise Refusal(
            f"repository path must stay inside {config.root} or name one direct sibling "
            f"with path components '../NAME'; other parent traversal is refused: {raw!r}"
        )
    normalized = Path(os.path.normpath(str(candidate)))
    if not normalized.parts:
        normalized = Path(".")
    if is_direct_sibling:
        stored = normalized.as_posix()
        unresolved = config.root.parent / parts[1]
        if unresolved == config.root:
            raise Refusal(
                f"repository path {raw!r} aliases the project root; use '.' instead"
            )
        allowed_root = config.root.parent
    else:
        stored = "." if normalized == Path(".") else normalized.as_posix()
        unresolved = config.root / normalized
        allowed_root = config.root
    return _resolved_repository_path(config, stored, unresolved, allowed_root)


def _stored_repository_path(config: Config, raw: str) -> tuple[str, Path]:
    candidate = Path(raw)
    if not candidate.is_absolute():
        return _repository_path(config, raw)
    if raw != os.path.normpath(raw):
        raise Refusal(
            f"stored absolute repository path is not canonical: {raw!r}"
        )
    if (
        candidate in {config.root, config.root.parent}
        or not candidate.name
        or candidate.parent != config.root.parent
    ):
        raise Refusal(
            "stored absolute repository path must name one direct sibling of the project "
            f"root, not the project root or another external path: {raw!r}"
        )
    stored = (Path("..") / candidate.name).as_posix()
    return _resolved_repository_path(config, stored, candidate, config.root.parent)


def _managed_worktrees_root(config: Config) -> Path:
    return config.worktrees.parent if config.worktrees.name == "slots" else config.worktrees


def _validate_slots_directory(config: Config) -> Path:
    return _managed_worktrees_root(config) / "validate"


def _slot_roots(config: Config) -> dict[str, Path]:
    return {
        "agent": config.worktrees,
        "validate": _validate_slots_directory(config),
    }


def _slot_directory(config: Config, slot: str, slot_type: str = "agent") -> Path:
    _validate_name(slot, "slot")
    if slot_type not in SLOT_TYPES:
        raise Refusal(f"slot type must be one of {', '.join(SLOT_TYPES)}")
    return _slot_roots(config)[slot_type] / slot


def _checkout_path(
    config: Config, slot: str, name: str, slot_type: str = "agent"
) -> tuple[str, Path]:
    return _checkout_path_for_layout(
        config, slot, name, slot_type, config.layout
    )


def _checkout_path_for_layout(
    config: Config,
    slot: str,
    name: str,
    slot_type: str,
    layout: str,
) -> tuple[str, Path]:
    _validate_name(slot, "slot")
    _validate_name(name, "checkout name")
    _validate_layout(layout)
    slot_path = _slot_directory(config, slot, slot_type)
    destination = slot_path if layout == "flat" else slot_path / name
    _ensure_no_symlink_components(config.root, destination, "checkout")
    relative = destination.relative_to(config.root).as_posix()
    return relative, destination


def _record_layout(config: Config, record: ActiveRecord) -> str:
    return config.layout if record.layout is None else record.layout


def _stored_path(config: Config, raw: str, label: str) -> Path:
    relative, absolute = _relative_inside(config.root, raw, label)
    del relative
    return absolute


def _boot_id(proc_root: Path) -> str:
    try:
        value = (proc_root / "sys/kernel/random/boot_id").read_text(encoding="ascii").strip()
    except (OSError, UnicodeError) as exc:
        raise Refusal(f"cannot read the machine boot id: {exc}") from exc
    if not value:
        raise Refusal("machine boot id is empty")
    return value


def _host_id() -> str:
    candidates = (Path("/etc/machine-id"), Path("/var/lib/dbus/machine-id"))
    failures: list[str] = []
    for path in candidates:
        try:
            value = path.read_text(encoding="ascii").strip()
        except FileNotFoundError:
            continue
        except (OSError, UnicodeError) as exc:
            failures.append(f"{path}: {exc}")
            continue
        if value:
            return value
        failures.append(f"{path}: empty")
    detail = f" ({'; '.join(failures)})" if failures else ""
    raise Refusal(f"cannot establish the stable machine identity{detail}")


def _read_process_identity(pid: int) -> ProcessIdentity:
    if pid <= 0:
        raise Refusal("owner PID must be positive")
    proc_root = Path("/proc")
    try:
        stat_text = (proc_root / str(pid) / "stat").read_text(encoding="ascii")
    except FileNotFoundError as exc:
        raise Refusal(f"owner PID {pid} is not live") from exc
    except (OSError, UnicodeError) as exc:
        raise Refusal(f"owner PID {pid} is indeterminate: {exc}") from exc
    close = stat_text.rfind(")")
    if close < 0:
        raise Refusal(f"owner PID {pid} has an unreadable process generation")
    fields = stat_text[close + 2 :].split()
    if len(fields) <= 19:
        raise Refusal(f"owner PID {pid} has an incomplete process generation")
    try:
        start_ticks = int(fields[19])
    except ValueError as exc:
        raise Refusal(f"owner PID {pid} has an invalid process generation") from exc
    if start_ticks <= 0:
        raise Refusal(f"owner PID {pid} has an invalid process generation")
    cgroup_path = _read_process_cgroup(proc_root / str(pid))
    return ProcessIdentity(
        pid=pid,
        start_ticks=start_ticks,
        boot_id=_boot_id(proc_root),
        host_id=_host_id(),
        cgroup_path=cgroup_path,
    )


def _read_process_cgroup(pid_dir: Path) -> str:
    try:
        lines = (pid_dir / "cgroup").read_text(encoding="ascii").splitlines()
    except (OSError, UnicodeError) as exc:
        raise Refusal(f"cannot read process cgroup for PID {pid_dir.name}: {exc}") from exc
    paths = [line.split("::", 1)[1] for line in lines if "::" in line]
    if len(paths) != 1 or not paths[0].startswith("/"):
        raise Refusal(f"cannot establish one unified cgroup for PID {pid_dir.name}")
    return paths[0].rstrip("/") or "/"


def _read_process_parent(pid: int) -> int | None:
    try:
        stat_text = Path("/proc") .joinpath(str(pid), "stat").read_text(encoding="ascii")
    except FileNotFoundError:
        return None
    except (OSError, UnicodeError) as exc:
        raise Refusal(f"cannot read process ancestry for PID {pid}: {exc}") from exc
    close = stat_text.rfind(")")
    fields = stat_text[close + 2 :].split() if close >= 0 else []
    if len(fields) < 2:
        raise Refusal(f"cannot parse process ancestry for PID {pid}")
    try:
        parent = int(fields[1])
    except ValueError as exc:
        raise Refusal(f"cannot parse process ancestry for PID {pid}") from exc
    return parent if parent > 0 and parent != pid else None


def _assert_caller_process(identity: ProcessIdentity, label: str) -> None:
    current: int | None = os.getpid()
    seen: set[int] = set()
    for _ in range(256):
        if current is None or current in seen:
            break
        seen.add(current)
        try:
            candidate = _read_process_identity(current)
        except Refusal:
            candidate = None
        if candidate == identity:
            return
        current = _read_process_parent(current)
    raise Refusal(
        f"{label} PID {identity.pid} is not in the invoking process ancestry"
    )


def _capture_caller_process(pid: int, label: str) -> ProcessIdentity:
    identity = _read_process_identity(pid)
    _assert_caller_process(identity, label)
    return identity


def _capture_registration_owner(pid: int) -> ProcessIdentity:
    """Capture an owner now; registration verifies its authority before publication."""
    return _read_process_identity(pid)


def _assert_process_descends_from(
    process: ProcessIdentity, ancestor: ProcessIdentity, label: str
) -> None:
    current: int | None = process.pid
    seen: set[int] = set()
    for _ in range(256):
        if current is None or current in seen:
            break
        seen.add(current)
        try:
            candidate = _read_process_identity(current)
        except Refusal:
            candidate = None
        if candidate == ancestor:
            return
        current = _read_process_parent(current)
    raise Refusal(
        f"{label} PID {process.pid} does not descend from coordinator PID "
        f"{ancestor.pid}"
    )


def _assert_registration_owner_authorized(
    owner: ProcessIdentity,
    coordinator: ProcessIdentity,
    slot_path: Path,
) -> None:
    try:
        _assert_caller_process(owner, "owner")
        return
    except Refusal as caller_refusal:
        _assert_process_descends_from(owner, coordinator, "owner")
        uses = _process_uses_slot(Path("/proc") / str(owner.pid), slot_path)
        if not any(item.startswith("cwd=") for item in uses):
            raise Refusal(
                f"owner PID {owner.pid} is outside the invoking process ancestry and "
                f"does not have its working directory inside slot {slot_path}"
            ) from caller_refusal


def _registered_liveness_state(config: Config, record: ActiveRecord) -> tuple[str, str]:
    env = {
        "LC_ALL": "C",
        "PATH": "/usr/bin:/bin",
        "WRKSLOTS_PROJECT_ROOT": str(config.root),
        "WRKSLOTS_SLOT": record.slot,
        "WRKSLOTS_SLOT_TYPE": record.slot_type,
        "WRKSLOTS_AGENT": record.agent,
        "WRKSLOTS_TASK": record.task,
        "WRKSLOTS_MACHINE": record.machine,
        "WRKSLOTS_GENERATION": str(record.generation),
        "WRKSLOTS_OWNER_PID": "" if record.owner is None else str(record.owner.pid),
        "WRKSLOTS_OWNER_START_TICKS": (
            "" if record.owner is None else str(record.owner.start_ticks)
        ),
        "WRKSLOTS_OWNER_BOOT_ID": "" if record.owner is None else record.owner.boot_id,
        "WRKSLOTS_OWNER_CGROUP": "" if record.owner is None else record.owner.cgroup_path,
    }
    try:
        command = (
            [sys.executable, str(config.liveness_command), record.agent]
            if config.liveness_command.suffix == ".py"
            else [str(config.liveness_command), record.agent]
        )
        completed = subprocess.run(
            command,
            text=True,
            capture_output=True,
            check=False,
            env=env,
        )
    except OSError as exc:
        return "unverifiable", f"registered liveness command could not run: {exc}"
    detail = (completed.stderr or completed.stdout).strip().splitlines()
    first = detail[0] if detail else "no detail"
    if completed.returncode == 0:
        return "dead", first
    if completed.returncode == 1:
        return "alive", first
    if completed.returncode == 2:
        return "unverifiable", first
    return "unverifiable", (
        f"registered liveness command returned unexpected rc {completed.returncode}: {first}"
    )


def _assert_registered_liveness(config: Config, record: ActiveRecord) -> None:
    state, detail = _registered_liveness_state(config, record)
    if state == "dead":
        return
    if state == "alive":
        raise Refusal(
            f"registered liveness authority reports owner alive: {detail}. state: "
            "REFUSED -- no checkout was salvaged or removed. remedy: let the owner "
            "finish or stop, then rerun remove after the time-to-live expires"
        )
    raise Refusal(
        f"registered liveness authority is unverifiable: {detail}. state: REFUSED -- "
        "no checkout was salvaged or removed; unknown ownership is not a free slot. "
        "remedy: refresh or repair the configured liveness source, then rerun remove"
    )


def _process_state(identity: ProcessIdentity | None) -> tuple[str, str]:
    if identity is None:
        return "indeterminate", "no owner process generation is recorded"
    try:
        host_id = _host_id()
    except Refusal as exc:
        return "indeterminate", str(exc)
    if host_id != identity.host_id:
        return (
            "indeterminate",
            "the recorded owner belongs to a different machine identity",
        )
    try:
        boot_id = _boot_id(Path("/proc"))
    except Refusal as exc:
        return "indeterminate", str(exc)
    if boot_id != identity.boot_id:
        return "dead", "the recorded machine boot ended"
    try:
        current = _read_process_identity(identity.pid)
    except Refusal as exc:
        message = str(exc)
        if "is not live" in message:
            return "dead", message
        return "indeterminate", message
    if current == identity:
        return "live", f"PID {identity.pid} generation is live"
    return "dead", f"PID {identity.pid} was reused; the recorded generation exited"


def _link_target(path: Path) -> Path | None:
    try:
        raw = os.readlink(path)
    except FileNotFoundError:
        return None
    except PermissionError as exc:
        raise Refusal(
            f"process use is indeterminate because {path} is unreadable: {exc}"
        ) from exc
    except OSError as exc:
        if exc.errno == errno.EINVAL:
            return None
        raise Refusal(f"process use is indeterminate because {path} failed: {exc}") from exc
    if raw.endswith(" (deleted)"):
        raw = raw[: -len(" (deleted)")]
    target = Path(raw)
    if not target.is_absolute():
        target = path.parent / target
    return Path(os.path.normpath(str(target)))


def _process_uses_slot(pid_dir: Path, slot_path: Path) -> list[str]:
    uses: list[str] = []
    for name in ("cwd", "root", "exe"):
        target = _link_target(pid_dir / name)
        if target is not None and _path_is_within(target, slot_path):
            uses.append(f"{name}={target}")
    fd_dir = pid_dir / "fd"
    try:
        descriptors = list(fd_dir.iterdir())
    except FileNotFoundError:
        descriptors = []
    except PermissionError as exc:
        raise Refusal(
            f"process use is indeterminate because {fd_dir} is unreadable: {exc}"
        ) from exc
    except OSError as exc:
        raise Refusal(
            f"process use is indeterminate because {fd_dir} cannot be read: {exc}"
        ) from exc
    for descriptor in descriptors:
        target = _link_target(descriptor)
        if target is not None and _path_is_within(target, slot_path):
            uses.append(f"fd/{descriptor.name}={target}")
            break
    maps_path = pid_dir / "maps"
    try:
        maps = maps_path.read_text(encoding="utf-8", errors="surrogateescape")
    except FileNotFoundError:
        maps = ""
    except PermissionError as exc:
        raise Refusal(
            f"process use is indeterminate because {maps_path} is unreadable: {exc}"
        ) from exc
    except OSError as exc:
        raise Refusal(
            f"process use is indeterminate because {maps_path} cannot be read: {exc}"
        ) from exc
    slot_text = str(slot_path)
    for line in maps.splitlines():
        if slot_text in line:
            fields = line.split(maxsplit=5)
            if len(fields) == 6:
                mapped = Path(fields[5].removesuffix(" (deleted)"))
                if mapped.is_absolute() and _path_is_within(mapped, slot_path):
                    uses.append(f"map={mapped}")
                    break
    uses.extend(_process_mounts_slot(pid_dir, slot_path))
    return uses


def _process_mounts_slot(pid_dir: Path, slot_path: Path) -> list[str]:
    uses: list[str] = []
    mountinfo_path = pid_dir / "mountinfo"
    try:
        mountinfo = mountinfo_path.read_text(
            encoding="utf-8", errors="surrogateescape"
        )
    except FileNotFoundError:
        mountinfo = ""
    except OSError as exc:
        if exc.errno in (errno.ENOENT, errno.ESRCH, errno.EINVAL):
            return []
        raise Refusal(
            f"process use is indeterminate because {mountinfo_path} is unreadable: {exc}"
        ) from exc
    for line in mountinfo.splitlines():
        left, separator, right = line.partition(" - ")
        if not separator:
            continue
        left_fields = left.split()
        right_fields = right.split()
        candidates = [
            *(left_fields[index] for index in (3, 4) if len(left_fields) > index),
            *(right_fields[index] for index in (1, 2) if len(right_fields) > index),
        ]
        for raw in candidates:
            candidate = Path(raw.replace("\\040", " ").removesuffix(" (deleted)"))
            if candidate.is_absolute() and _path_is_within(candidate, slot_path):
                uses.append(f"mount={candidate}")
                return uses
    return uses


def _mount_namespace(pid_dir: Path) -> str | None:
    path = pid_dir / "ns" / "mnt"
    try:
        return os.readlink(path)
    except FileNotFoundError:
        return None
    except PermissionError:
        return f"pid:{pid_dir.name}"
    except OSError as exc:
        if exc.errno in (errno.ENOENT, errno.ESRCH, errno.EINVAL):
            return None
        raise Refusal(
            f"mount namespace is indeterminate because {path} is unreadable: {exc}"
        ) from exc


def _process_uid(pid_dir: Path) -> int | None:
    try:
        status = (pid_dir / "status").read_text(encoding="ascii")
    except FileNotFoundError:
        return None
    except (OSError, UnicodeError) as exc:
        raise Refusal(
            f"process ownership is indeterminate because {pid_dir / 'status'} is unreadable: {exc}"
        ) from exc
    for line in status.splitlines():
        if line.startswith("Uid:"):
            fields = line.split()
            if len(fields) < 2:
                break
            try:
                return int(fields[1])
            except ValueError:
                break
    raise Refusal(f"process ownership is indeterminate for {pid_dir.name}")


def _unrelated_lsof_warnings(stderr: str, slot_path: Path) -> bool:
    lines = [line for line in stderr.splitlines() if line.strip()]
    if not lines:
        return True
    patterns = (
        re.compile(r"^lsof: WARNING: can't stat\(\) .* file system (?P<path>/.*)$"),
        re.compile(r"^lsof: WARNING: can't opendir\((?P<path>/.*)\): .*$"),
    )
    for line in lines:
        if line.strip() == "Output information may be incomplete.":
            continue
        matches = [match for pattern in patterns if (match := pattern.fullmatch(line))]
        raw_paths = (
            [matches[0].group("path")]
            if matches
            else re.findall(r"/[^\s)]+", line)
        )
        if len(raw_paths) != 1:
            return False
        unreadable = Path(raw_paths[0].rstrip(":"))
        if _path_is_within(slot_path, unreadable) or _path_is_within(unreadable, slot_path):
            return False
    return True


def _assert_slot_unused(
    slot_path: Path,
    record: ActiveRecord | None = None,
    *,
    use_lsof: bool = True,
    ignore_invoking_ancestry: bool = False,
) -> None:
    try:
        current_directory = Path.cwd().resolve(strict=True)
    except OSError as exc:
        raise Refusal(f"cannot establish the current working directory: {exc}") from exc
    if _path_is_within(current_directory, slot_path):
        raise Refusal(
            f"the wrkslots process is running from inside slot {slot_path}; change directory and retry"
        )
    lsof = next(
        (
            candidate
            for candidate in (Path("/usr/bin/lsof"), Path("/usr/sbin/lsof"))
            if candidate.is_file() and os.access(candidate, os.X_OK)
        ),
        None,
    )
    direct_use_proven_absent = False
    if lsof is not None and use_lsof:
        try:
            completed = subprocess.run(
                [str(lsof), "-nP", "-Fpcfn", "+D", str(slot_path)],
                text=True,
                capture_output=True,
                check=False,
            )
        except OSError as exc:
            raise Refusal(
                f"process use is indeterminate because lsof failed: {exc}"
            ) from exc
        if not _unrelated_lsof_warnings(completed.stderr, slot_path):
            raise Refusal(
                "process use is indeterminate because lsof reported: "
                f"{completed.stderr.strip().splitlines()[0]}"
            )
        pids = sorted(
            {
                int(line[1:])
                for line in completed.stdout.splitlines()
                if line.startswith("p") and line[1:].isdigit()
            }
        )
        if pids:
            raise Refusal(
                f"live process {pids[0]} uses slot {slot_path}; stop it and retry"
            )
        if completed.returncode not in (0, 1):
            raise Refusal(
                f"process use is indeterminate because lsof exited {completed.returncode}"
            )
        direct_use_proven_absent = True
    proc_root = Path("/proc")
    try:
        pid_dirs = [path for path in proc_root.iterdir() if path.name.isdigit()]
    except OSError as exc:
        raise Refusal(f"cannot inspect live processes: {exc}") from exc
    current_pid = os.getpid()
    invoking_ancestry: set[int] = set()
    if ignore_invoking_ancestry:
        ancestor: int | None = current_pid
        for _ in range(256):
            if ancestor is None or ancestor in invoking_ancestry:
                break
            invoking_ancestry.add(ancestor)
            ancestor = _read_process_parent(ancestor)
    inspected_mount_namespaces: set[str] = set()
    for pid_dir in sorted(pid_dirs, key=lambda item: int(item.name)):
        pid = int(pid_dir.name)
        if pid == current_pid or pid in invoking_ancestry:
            continue
        uid = _process_uid(pid_dir)
        if uid is None:
            continue
        if record is not None and record.owner is not None:
            try:
                cgroup = _read_process_cgroup(pid_dir)
            except Refusal as exc:
                raise Refusal(
                    f"recorded cgroup use is indeterminate for PID {pid}: {exc}"
                ) from exc
            if cgroup == record.owner.cgroup_path:
                raise Refusal(
                    f"live process {pid} remains in recorded owner cgroup {cgroup}"
                )
        try:
            if direct_use_proven_absent:
                mount_namespace = _mount_namespace(pid_dir)
                if mount_namespace is None or mount_namespace in inspected_mount_namespaces:
                    uses = []
                else:
                    uses = _process_mounts_slot(pid_dir, slot_path)
                    inspected_mount_namespaces.add(mount_namespace)
            else:
                uses = _process_uses_slot(pid_dir, slot_path)
        except Refusal:
            if not use_lsof:
                _assert_slot_unused(
                    slot_path,
                    record,
                    use_lsof=True,
                    ignore_invoking_ancestry=ignore_invoking_ancestry,
                )
                return
            raise
        if uses:
            raise Refusal(
                f"live process {pid} uses slot {slot_path}: {uses[0]}; stop it and retry"
            )


def _parse_timestamp(value: str, label: str) -> dt.datetime:
    try:
        parsed = dt.datetime.fromisoformat(value)
    except ValueError as exc:
        raise StateError(f"invalid timestamp in {label}: {value!r}") from exc
    if parsed.tzinfo is None:
        raise StateError(f"timestamp in {label} must include a time zone")
    return parsed


def _heartbeat_diagnosis(record: ActiveRecord) -> tuple[float, bool]:
    stamp = _parse_timestamp(record.heartbeat_at, f"slot {record.slot} heartbeat")
    age = max(0.0, (dt.datetime.now(dt.timezone.utc) - stamp).total_seconds())
    return age, age > record.heartbeat_ttl_seconds


def _assert_record_paths(config: Config, record: ActiveRecord) -> None:
    expected_slot = _slot_directory(config, record.slot, record.slot_type)
    _ensure_no_symlink_components(config.root, expected_slot, "slot")
    layout = _record_layout(config, record)
    if layout == "flat" and len(record.checkouts) != 1:
        raise StateError(f"slot {record.slot} has multiple checkouts under flat layout")
    for checkout in record.checkouts:
        if checkout.landed_ref != _landed_ref_for_remote(config, checkout.remote):
            raise StateError(
                f"slot {record.slot} checkout {checkout.name} differs from configured authority"
            )
        expected_relative, expected_absolute = _checkout_path_for_layout(
            config, record.slot, checkout.name, record.slot_type, layout
        )
        if checkout.path != expected_relative:
            raise StateError(
                f"slot {record.slot} checkout {checkout.name} path does not match its slot: "
                f"{checkout.path!r} != {expected_relative!r}"
            )
        actual = _stored_path(config, checkout.path, "checkout path")
        if actual != expected_absolute:
            raise StateError(f"slot {record.slot} checkout path identity changed")
        _stored_repository_path(config, checkout.repository)


def _assert_checkout_identity_unchanged(
    config: Config, checkout: Checkout, vcs: _GitVcs
) -> None:
    path = _stored_path(config, checkout.path, "checkout path")
    _relative, repository = _stored_repository_path(config, checkout.repository)
    head = vcs.verify_existing_worktree(repository, path)
    branch = vcs.branch(path)
    remote_url_sha256 = vcs.remote_url_sha256(path, checkout.remote)
    if (
        head != checkout.head
        or branch != checkout.branch
        or remote_url_sha256 != checkout.remote_url_sha256
    ):
        raise Refusal(
            f"checkout {checkout.name} identity changed before state publication; "
            "preserve it for inspection"
        )


def _unpushed_evidence(
    checkout: Checkout, path: Path, head: str, vcs: _GitVcs
) -> dict[str, object]:
    same_named_ref = f"refs/remotes/{checkout.remote}/{checkout.branch}"
    exists = vcs.ref_exists(path, same_named_ref)
    command: str | None = None
    touched = vcs.commit_paths(path, head)
    if exists:
        command = shlex.join(
            [
                "git",
                "--literal-pathspecs",
                "-C",
                str(path),
                "diff",
                head,
                same_named_ref,
                "--",
                *touched,
            ]
        )
    return {
        "same_named_remote_ref": same_named_ref if exists else None,
        "touched_files": list(touched),
        "diagnostic_command": command,
    }


def _unpushed_refusal(checkout: Checkout, path: Path, head: str, vcs: _GitVcs) -> str:
    evidence = _unpushed_evidence(checkout, path, head, vcs)
    same_named_ref = evidence["same_named_remote_ref"]
    base = (
        f"checkout {checkout.name} HEAD {head} is not reachable from any "
        f"refs/remotes/{checkout.remote}/* ref"
    )
    if same_named_ref is None:
        return (
            f"{base}. A same-named remote ref does not exist; publish the branch before finishing"
        )
    command = _as_str(evidence["diagnostic_command"], "unpushed diagnostic command")
    return (
        f"{base}. Same-named remote ref {same_named_ref} exists, so there are two "
        "opposite readings: this commit is genuinely unpushed, or the remote branch "
        "was rebased and this local tip is stale. Do not force-push based on this "
        f"refusal. Run `{command}`; empty output means the touched content is already "
        "present at the remote tip"
    )


def _assert_cache_policy_untracked_path(
    config: Config,
    checkout_name: str,
    path: Path,
    vcs: _GitVcs,
    cache_globs: Sequence[str],
) -> None:
    if not cache_globs:
        return
    tracked = vcs.tracked_cache_paths(path, cache_globs)
    if tracked:
        raise Refusal(
            f"configured cache policy for checkout {checkout_name} overlaps tracked source "
            f"at {tracked[0]}; narrow cache_globs before cleanup or finish"
        )
    for submodule in vcs.submodule_paths(path):
        submodule_parts = Path(submodule).parts
        for pattern in cache_globs:
            pattern_parts = Path(pattern).parts
            wildcard_index = next(
                (
                    index
                    for index, part in enumerate(pattern_parts)
                    if any(character in part for character in "*?[")
                ),
                len(pattern_parts),
            )
            literal_prefix = pattern_parts[:wildcard_index]
            if wildcard_index == len(pattern_parts):
                overlaps = (
                    submodule_parts[: len(pattern_parts)] == pattern_parts
                    or pattern_parts[: len(submodule_parts)] == submodule_parts
                )
            else:
                overlaps = (
                    submodule_parts[: len(literal_prefix)] == literal_prefix
                    or literal_prefix[: len(submodule_parts)] == submodule_parts
                )
            if overlaps:
                raise Refusal(
                    f"configured cache policy for checkout {checkout_name} may cross "
                    f"Git submodule {submodule}; configure that repository separately"
                )


def _assert_cache_policy_untracked(
    config: Config, checkout: Checkout, vcs: _GitVcs
) -> None:
    path = _stored_path(config, checkout.path, "checkout path")
    _assert_cache_policy_untracked_path(
        config, checkout.name, path, vcs, _cache_globs_for(config, checkout.name)
    )


def _assert_checkout_safe(
    config: Config,
    checkout: Checkout,
    vcs: _GitVcs,
    *,
    refresh_remote: bool = True,
) -> Checkout:
    path = _stored_path(config, checkout.path, "checkout path")
    _relative, repository = _stored_repository_path(config, checkout.repository)
    head = vcs.verify_existing_worktree(repository, path)
    branch = vcs.branch(path)
    if branch != checkout.branch:
        raise Refusal(
            f"checkout {checkout.name} branch changed: expected {checkout.branch}, found {branch}"
        )
    operation_paths = vcs.operation_paths(path)
    if operation_paths:
        raise Refusal(
            f"checkout {checkout.name} has an unfinished Git operation: {operation_paths[0]}"
        )
    vcs.assert_ordinary_history(path)
    vcs.assert_ordinary_index(path)
    _assert_cache_policy_untracked_path(
        config,
        checkout.name,
        path,
        vcs,
        _cache_globs_for(config, checkout.name),
    )
    status = vcs.status(path, _cache_globs_for(config, checkout.name))
    if status:
        first = status.splitlines()[0]
        raise Refusal(
            f"checkout {checkout.name} is dirty or has untracked/ignored files ({first}); "
            "commit intended work and publish it before finishing"
        )
    remote_url_sha256 = vcs.remote_url_sha256(path, checkout.remote)
    if remote_url_sha256 != checkout.remote_url_sha256:
        raise Refusal(
            f"checkout {checkout.name} remote {checkout.remote} URL changed; "
            "preserve the checkout for inspection"
        )
    if refresh_remote:
        vcs.fetch_remote(path, checkout.remote, checkout.landed_ref)
        if vcs.remote_url_sha256(path, checkout.remote) != checkout.remote_url_sha256:
            raise Refusal(
                f"checkout {checkout.name} remote {checkout.remote} URL changed during fetch; "
                "preserve the checkout for inspection"
            )
    remote_refs = vcs.remote_refs_containing(path, checkout.remote, head)
    if not remote_refs:
        raise Refusal(_unpushed_refusal(checkout, path, head, vcs))
    if not vcs.is_ancestor(path, head, checkout.landed_ref):
        raise Refusal(
            f"checkout {checkout.name} HEAD {head} is not an ancestor of configured landed ref "
            f"{checkout.landed_ref}; no PR label or status substitutes for ancestry"
        )
    return dataclasses.replace(
        checkout,
        head=head,
        containing_remote_refs=remote_refs,
    )


def _salvage_pathspecs(config: Config, checkout_name: str) -> list[str]:
    pathspecs = ["."]
    for pattern in _cache_globs_for(config, checkout_name):
        pathspecs.append(f":(exclude,glob){pattern}")
        pathspecs.append(f":(exclude,glob){pattern}/**")
    return pathspecs


def _salvage_candidate(
    config: Config,
    record: ActiveRecord,
    checkout: Checkout,
    vcs: _GitVcs,
    *,
    path_override: Path | None = None,
    allow_detached: bool = False,
    verify_repository_membership: bool = True,
) -> tuple[str, str, str, bool]:
    """Return HEAD, status digest, durable commit candidate, and dirty state.

    A temporary index captures tracked, untracked, and ignored files outside the
    configured regenerable caches. It never changes the checkout's branch or
    ordinary index, so a failed push leaves the author's tree untouched.
    """

    path = path_override or _stored_path(config, checkout.path, "checkout path")
    _relative, repository = _stored_repository_path(config, checkout.repository)
    if verify_repository_membership:
        head = vcs.verify_existing_worktree(repository, path)
    else:
        if vcs.repository_root(path) != path.absolute():
            raise Refusal(f"nested repository is not rooted at its recorded path: {path}")
        head = vcs.head(path)
    if allow_detached:
        branch_result = vcs._run(
            path, ["symbolic-ref", "--quiet", "--short", "HEAD"], check=False
        )
        if branch_result.returncode not in (0, 1):
            raise Refusal(f"cannot determine branch state for checkout {checkout.name}")
    else:
        branch = vcs.branch(path)
        if branch != checkout.branch:
            raise Refusal(
                f"checkout {checkout.name} branch changed: expected {checkout.branch}, "
                f"found {branch}; preserve it and update the recorded checkout before reclaim"
            )
    operations = vcs.operation_paths(path)
    if operations:
        raise Refusal(
            f"checkout {checkout.name} has an unfinished Git operation at {operations[0]}; "
            "complete or abort that Git operation before reclaim"
        )
    vcs.assert_ordinary_history(path)
    vcs.assert_ordinary_index(path)
    _assert_cache_policy_untracked_path(
        config,
        checkout.name,
        path,
        vcs,
        _cache_globs_for(config, checkout.name),
    )
    if vcs.remote_url_sha256(path, checkout.remote) != checkout.remote_url_sha256:
        raise Refusal(
            f"checkout {checkout.name} remote {checkout.remote} URL changed; restore the "
            "recorded remote before reclaim so salvage cannot be sent elsewhere"
        )
    status = vcs.status(path, _cache_globs_for(config, checkout.name))
    status_digest = hashlib.sha256(status.encode("utf-8")).hexdigest()
    if not status:
        return head, status_digest, head, False

    with tempfile.TemporaryDirectory(prefix="wrkslots-salvage-") as temporary:
        index = str(Path(temporary) / "index")
        env = {"GIT_INDEX_FILE": index}
        vcs._run(path, ["read-tree", "HEAD"], env_overrides=env)
        pathspecs = _salvage_pathspecs(config, checkout.name)
        vcs._run(path, ["add", "-u", "--", *pathspecs], env_overrides=env)
        untracked = vcs._run(
            path,
            [
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                *pathspecs,
            ],
            env_overrides=env,
        ).stdout
        if untracked:
            vcs._run(
                path,
                ["add", "--pathspec-from-file=-", "--pathspec-file-nul"],
                input_text=untracked,
                env_overrides=env,
            )
        ignored = vcs._run(
            path,
            [
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
                "--",
                *pathspecs,
            ],
            env_overrides=env,
        ).stdout
        if ignored:
            vcs._run(
                path,
                ["add", "-f", "--pathspec-from-file=-", "--pathspec-file-nul"],
                input_text=ignored,
                env_overrides=env,
            )
        tree = vcs._run(path, ["write-tree"], env_overrides=env).stdout.strip()
        if not SHA_RE.fullmatch(tree):
            raise Refusal(f"salvage tree for checkout {checkout.name} is not a full Git object")
        message = (
            f"Salvage {record.slot}/{checkout.name} after recorded owner exit\n\n"
            f"task: {record.task}\n"
            f"source-head: {head}\n"
        )
        identity = {
            "GIT_AUTHOR_NAME": "wrkslots salvage",
            "GIT_AUTHOR_EMAIL": "wrkslots@localhost",
            "GIT_AUTHOR_DATE": record.heartbeat_at,
            "GIT_COMMITTER_NAME": "wrkslots salvage",
            "GIT_COMMITTER_EMAIL": "wrkslots@localhost",
            "GIT_COMMITTER_DATE": record.heartbeat_at,
        }
        commit = vcs._run(
            path,
            ["commit-tree", tree, "-p", head],
            input_text=message,
            env_overrides=identity,
        ).stdout.strip()
    if not SHA_RE.fullmatch(commit):
        raise Refusal(f"salvage commit for checkout {checkout.name} is not a full Git object")
    return head, status_digest, commit, True


def _submodule_salvage_checkouts(
    config: Config,
    checkout: Checkout,
    vcs: _GitVcs,
    *,
    path_override: Path | None = None,
) -> tuple[tuple[Checkout, Path], ...]:
    checkout_path = path_override or _stored_path(
        config, checkout.path, "checkout path"
    )
    _repository_relative, source_repository = _stored_repository_path(
        config, checkout.repository
    )
    result: list[tuple[Checkout, Path]] = []
    for relative_value in vcs.initialized_submodules(checkout_path):
        relative = Path(relative_value)
        managed = checkout_path / relative
        source = source_repository / relative
        _ensure_no_symlink_components(checkout_path, managed, "submodule checkout")
        _ensure_no_symlink_components(config.root, source, "source submodule")
        if not source.is_dir() or source.is_symlink():
            raise Refusal(
                f"source submodule is unavailable for {checkout.name}/{relative_value}: "
                f"{source}. Preserve the slot until its remote authority can be established"
            )
        if vcs.repository_root(source) != source.absolute():
            raise Refusal(f"source submodule is not a Git root: {source}")
        remote = config.default_remote
        authorized_remote = vcs.remote_url_sha256(source, remote)
        if vcs.remote_url_sha256(managed, remote) != authorized_remote:
            raise Refusal(
                f"submodule {checkout.name}/{relative_value} remote {remote} differs "
                "from its configured source; preserve the slot for inspection"
            )
        head = vcs.head(managed)
        stored_path = (Path(checkout.path) / relative).as_posix()
        source_relative = source.relative_to(config.root).as_posix()
        result.append(
            (
                Checkout(
                    name=f"{checkout.name}/{relative_value}",
                    path=stored_path,
                    repository=source_relative,
                    branch="",
                    start_point=head,
                    remote=remote,
                    remote_url_sha256=authorized_remote,
                    landed_ref=_landed_ref_for_remote(config, remote),
                    head=head,
                ),
                managed,
            )
        )
    return tuple(result)


def _salvage_one_checkout(
    config: Config,
    record: ActiveRecord,
    checkout: Checkout,
    vcs: _GitVcs,
    *,
    path_override: Path | None = None,
    allow_detached: bool = False,
) -> dict[str, object]:
    path = path_override or _stored_path(config, checkout.path, "checkout path")
    head, status_digest, commit, dirty = _salvage_candidate(
        config,
        record,
        checkout,
        vcs,
        path_override=path,
        allow_detached=allow_detached,
        verify_repository_membership=not allow_detached,
    )
    vcs.fetch_remote(path, checkout.remote, checkout.landed_ref)
    if vcs.remote_url_sha256(path, checkout.remote) != checkout.remote_url_sha256:
        raise Refusal(
            f"checkout {checkout.name} remote {checkout.remote} URL changed during fetch; "
            "preserve the checkout and restore the recorded remote"
        )
    containing = vcs.remote_refs_containing(path, checkout.remote, head)
    if not dirty and containing:
        return {
            "checkout": checkout.name,
            "source_head": head,
            "salvage_commit": commit,
            "status_sha256": status_digest,
            "disposition": "already-published",
            "remote_ref": None,
            "containing_remote_refs": list(containing),
        }
    salvage_name = checkout.name.replace("/", "-")
    salvage_ref = (
        f"refs/heads/salvage/{record.machine}/{record.slot}/"
        f"{salvage_name}-{record.generation}-{commit[:12]}"
    )
    _validate_full_ref(salvage_ref, "salvage ref")
    vcs.push_salvage(path, checkout.remote, commit, salvage_ref)
    return {
        "checkout": checkout.name,
        "source_head": head,
        "salvage_commit": commit,
        "status_sha256": status_digest,
        "disposition": "salvaged",
        "remote_ref": salvage_ref,
        "containing_remote_refs": [],
    }


def _salvage_agent_slot(
    config: Config, record: ActiveRecord, vcs: _GitVcs
) -> tuple[dict[str, object], ...]:
    receipts: list[dict[str, object]] = []
    for checkout in record.checkouts:
        receipts.append(_salvage_one_checkout(config, record, checkout, vcs))
        for submodule, path in _submodule_salvage_checkouts(config, checkout, vcs):
            receipts.append(
                _salvage_one_checkout(
                    config,
                    record,
                    submodule,
                    vcs,
                    path_override=path,
                    allow_detached=True,
                )
            )
    return tuple(receipts)


def _assert_salvage_still_matches(
    config: Config,
    record: ActiveRecord,
    receipts: Sequence[Mapping[str, object]],
    vcs: _GitVcs,
    *,
    paths: Mapping[str, Path] | None = None,
) -> None:
    by_name = {
        _as_str(value.get("checkout"), "salvage receipt.checkout"): value
        for value in receipts
    }
    targets: list[tuple[Checkout, Path, bool]] = []
    for checkout in record.checkouts:
        path = (
            _stored_path(config, checkout.path, "checkout path")
            if paths is None
            else paths[checkout.name]
        )
        targets.append((checkout, path, False))
        targets.extend(
            (submodule, submodule_path, True)
            for submodule, submodule_path in _submodule_salvage_checkouts(
                config, checkout, vcs, path_override=path
            )
        )
    if set(by_name) != {checkout.name for checkout, _path, _detached in targets}:
        raise StateError("salvage receipts do not name every recorded checkout exactly once")
    for checkout, path, allow_detached in targets:
        receipt = by_name[checkout.name]
        head, status_digest, commit, _dirty = _salvage_candidate(
            config,
            record,
            checkout,
            vcs,
            path_override=path,
            allow_detached=allow_detached,
            verify_repository_membership=not allow_detached,
        )
        expected = (
            _as_str(receipt.get("source_head"), "salvage receipt.source_head"),
            _as_str(receipt.get("status_sha256"), "salvage receipt.status_sha256"),
            _as_str(receipt.get("salvage_commit"), "salvage receipt.salvage_commit"),
        )
        if (head, status_digest, commit) != expected:
            raise Refusal(
                f"checkout {checkout.name} changed after salvage; preserve it and rerun remove "
                "so the changed state is published before deletion"
            )
        disposition = _as_str(
            receipt.get("disposition"), "salvage receipt.disposition"
        )
        if disposition == "salvaged":
            remote_ref = _as_str(receipt.get("remote_ref"), "salvage receipt.remote_ref")
            if vcs.remote_ref_sha(path, checkout.remote, remote_ref) != commit:
                raise Refusal(
                    f"remote salvage ref {remote_ref} no longer contains {commit}; "
                    "preserve the checkout and restore the remote evidence before deletion"
                )
        elif disposition == "already-published":
            vcs.fetch_remote(path, checkout.remote, checkout.landed_ref)
            if not vcs.remote_refs_containing(path, checkout.remote, head):
                raise Refusal(
                    f"checkout {checkout.name} is no longer present on its recorded remote; "
                    "preserve it and rerun remove to publish salvage"
                )
        else:
            raise StateError(f"unknown salvage disposition {disposition!r}")


def _assert_slot_contents(config: Config, record: ActiveRecord) -> Path:
    return _assert_slot_contents_values(
        config,
        record.slot,
        record.slot_type,
        _record_layout(config, record),
        record.checkouts,
    )


def _assert_slot_contents_values(
    config: Config,
    slot: str,
    slot_type: str,
    layout: str,
    checkouts: Sequence[Checkout],
) -> Path:
    slot_path = _slot_directory(config, slot, slot_type)
    if not slot_path.is_dir() or slot_path.is_symlink():
        raise Refusal(f"slot directory is missing or unsafe: {slot_path}")
    if layout == "flat":
        if len(checkouts) != 1:
            raise StateError(f"slot {slot} does not have exactly one checkout under flat layout")
        checkout = checkouts[0]
        expected_path, _destination = _checkout_path_for_layout(
            config, slot, checkout.name, slot_type, layout
        )
        if checkout.path != expected_path:
            raise StateError(
                f"slot {slot} checkout path does not equal the flat slot root"
            )
        return slot_path
    expected_names = {checkout.name for checkout in checkouts}
    allowed_names = set(expected_names)
    if slot_type == "agent":
        allowed_names.add("HANDOFF.md")
    try:
        actual = {entry.name for entry in slot_path.iterdir()}
    except OSError as exc:
        raise Refusal(f"cannot inspect slot directory {slot_path}: {exc}") from exc
    extras = sorted(actual - allowed_names)
    missing = sorted(expected_names - actual)
    if extras or missing:
        details: list[str] = []
        if extras:
            details.append(f"unexpected entries {', '.join(extras)}")
        if missing:
            details.append(f"missing checkouts {', '.join(missing)}")
        raise Refusal(f"slot directory does not match its record: {'; '.join(details)}")
    return slot_path


def _assert_handoff_read(config: Config, record: ActiveRecord, slot_path: Path) -> None:
    if record.slot_type != "agent":
        return
    handoff = slot_path / "HANDOFF.md"
    if not handoff.exists() and not handoff.is_symlink():
        return
    if handoff.is_symlink() or not handoff.is_file():
        raise Refusal(
            f"slot {record.slot} has an unsafe HANDOFF.md. state: REFUSED -- no checkout "
            "was salvaged or removed. remedy: replace it with a regular file inside the slot, "
            "then run 'wrkslots read-handoff' before retrying remove"
        )
    try:
        contents = handoff.read_bytes()
    except OSError as exc:
        raise Refusal(
            f"slot {record.slot} HANDOFF.md could not be read: {exc}. state: REFUSED -- "
            "no checkout was salvaged or removed. remedy: make the file readable, run "
            "'wrkslots read-handoff', then retry remove"
        ) from exc
    digest = hashlib.sha256(contents).hexdigest()
    if _recorded_handoff_digest(config, record) != digest:
        raise Refusal(
            f"slot {record.slot} contains an unread HANDOFF.md. state: REFUSED -- no "
            "checkout was salvaged or removed. remedy: run 'wrkslots read-handoff "
            f"{record.slot} --coordinator-authorized --coordinator-pid <current-coordinator-pid>' "
            "and read its output, then retry remove"
        )


def _handoff_preconditions(
    config: Config,
    record: ActiveRecord,
    vcs: _GitVcs,
    *,
    refresh_remote: bool = True,
) -> tuple[Path, tuple[Checkout, ...]]:
    _assert_record_paths(config, record)
    slot_path = _assert_slot_contents(config, record)
    final_checkouts = tuple(
        _assert_checkout_safe(
            config, item, vcs, refresh_remote=refresh_remote
        )
        for item in record.checkouts
    )
    return slot_path, final_checkouts


def _remove_preconditions(
    config: Config, record: ActiveRecord, vcs: _GitVcs
) -> tuple[Path, tuple[Checkout, ...]]:
    if record.handoff is None:
        raise Refusal(
            f"slot {record.slot} has no owner-alive handoff; preserve it and record one before removal"
        )
    slot_path, final_checkouts = _handoff_preconditions(config, record, vcs)
    if final_checkouts != record.checkouts:
        raise Refusal(
            f"slot {record.slot} changed after its handoff was recorded; preserve it for inspection"
        )
    _assert_slot_unused(slot_path, record)
    return slot_path, final_checkouts


def _reclaim_preconditions(
    config: Config,
    record: ActiveRecord,
    salvage: Sequence[Mapping[str, object]],
    vcs: _GitVcs,
    *,
    allow_live_validate_owner: bool = False,
) -> tuple[Path, tuple[Checkout, ...]]:
    _assert_record_paths(config, record)
    slot_path = _assert_slot_contents(config, record)
    _assert_handoff_read(config, record, slot_path)
    _assert_slot_unused(
        slot_path,
        None if allow_live_validate_owner and record.slot_type == "validate" else record,
        use_lsof=not (allow_live_validate_owner and record.slot_type == "validate"),
        ignore_invoking_ancestry=(
            allow_live_validate_owner and record.slot_type == "validate"
        ),
    )
    final: list[Checkout] = []
    for checkout in record.checkouts:
        path = _stored_path(config, checkout.path, "checkout path")
        _relative, repository = _stored_repository_path(config, checkout.repository)
        head = vcs.verify_existing_worktree(repository, path)
        branch = vcs.branch(path)
        if branch != checkout.branch:
            raise Refusal(
                f"checkout {checkout.name} branch changed: expected {checkout.branch}, "
                f"found {branch}; preserve it and update the record before reclaim"
            )
        if record.slot_type == "agent":
            operations = vcs.operation_paths(path)
            if operations:
                raise Refusal(
                    f"checkout {checkout.name} has an unfinished Git operation at "
                    f"{operations[0]}; complete or abort it before reclaim"
                )
            vcs.assert_ordinary_history(path)
            vcs.assert_ordinary_index(path)
        if vcs.remote_url_sha256(path, checkout.remote) != checkout.remote_url_sha256:
            raise Refusal(
                f"checkout {checkout.name} remote {checkout.remote} URL changed; "
                "restore the recorded remote before reclaim"
            )
        final.append(dataclasses.replace(checkout, head=head))
    if record.slot_type == "validate":
        if salvage:
            raise StateError("validate slot unexpectedly records salvage evidence")
    elif salvage:
        _assert_salvage_still_matches(config, record, salvage, vcs)
    elif record.handoff is not None:
        return _remove_preconditions(config, record, vcs)
    else:
        raise StateError("agent slot reclaim has neither salvage nor an owner handoff")
    return slot_path, tuple(final)


def _refuse_partial_state(config: Config) -> None:
    leftovers = sorted(config.control.glob("ACTIVE.*.json.tmp.*"))
    leftovers += sorted(config.control.glob("ARCHIVED.*.json.tmp.*"))
    leftovers += sorted(config.control.glob("ACTIVE.*.journal.tmp.*"))
    for event_directory in sorted(config.control.glob("EVENTS.*")):
        if event_directory.is_dir() and not event_directory.is_symlink():
            leftovers += sorted(event_directory.glob("*.json.tmp.*"))
    if leftovers:
        raise StateError(
            f"partial atomic update found: {leftovers[0]}; preserve it and use 'wrkslots recover'"
        )


def _outstanding_journals(config: Config) -> list[Path]:
    paths = {path for path in config.control.glob("ACTIVE.*.journal")}
    for directory in config.control.glob("EVENTS.*"):
        if not directory.is_dir() or directory.is_symlink():
            continue
        machine = directory.name.removeprefix("EVENTS.")
        if _pending_operation_from_events(config, machine) is not None:
            paths.add(_journal_path(config, machine))
    return sorted(paths)


def _assert_no_journal(config: Config) -> None:
    journals = _outstanding_journals(config)
    if journals:
        raise Refusal(
            f"interrupted mutation recorded in {journals[0]}; run 'wrkslots recover' first"
        )


def _load_journal(config: Config) -> tuple[Path, Mapping[str, object]]:
    journals = _outstanding_journals(config)
    if not journals:
        raise Refusal("no interrupted mutation is recorded")
    if len(journals) != 1:
        raise StateError(
            "multiple interrupted mutations are present; inspect every journal before continuing"
        )
    path = journals[0]
    if path.exists() or path.is_symlink():
        raw = _as_mapping(_read_json(path, "recovery journal"), "recovery journal")
    else:
        machine = path.name.removeprefix("ACTIVE.").removesuffix(".journal")
        pending = _pending_operation_from_events(config, machine)
        if pending is None:
            raise StateError(
                f"append-only log names no pending operation for missing journal {path}"
            )
        raw = pending
    return path, raw


def _interrupt_for_test(point: str) -> None:
    if os.environ.get("WRKSLOTS_TEST_INTERRUPT") == point:
        os._exit(86)


def _assignments(values: Sequence[str] | None, label: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for raw in values or ():
        name, separator, value = raw.partition("=")
        if not separator or not value:
            raise Refusal(f"{label} must use NAME=VALUE, got {raw!r}")
        _validate_name(name, f"{label} name")
        if name in result:
            raise Refusal(f"duplicate {label} for {name!r}")
        result[name] = value
    return result


def _planned_to_obj(item: PlannedCheckout) -> dict[str, object]:
    return dataclasses.asdict(item)


def _planned_from_obj(value: object, label: str) -> PlannedCheckout:
    raw = _as_mapping(value, label)
    fields = {
        "name",
        "destination",
        "repository",
        "branch",
        "start_point",
        "remote",
        "remote_url_sha256",
        "landed_ref",
    }
    _exact_keys(raw, fields, set(), label)
    item = PlannedCheckout(
        name=_as_str(raw["name"], f"{label}.name"),
        destination=_as_str(raw["destination"], f"{label}.destination"),
        repository=_as_str(raw["repository"], f"{label}.repository"),
        branch=_as_str(raw["branch"], f"{label}.branch"),
        start_point=_as_str(raw["start_point"], f"{label}.start_point"),
        remote=_as_str(raw["remote"], f"{label}.remote"),
        remote_url_sha256=_as_str(
            raw["remote_url_sha256"], f"{label}.remote_url_sha256"
        ),
        landed_ref=_as_str(raw["landed_ref"], f"{label}.landed_ref"),
    )
    _validate_name(item.name, "checkout name")
    _validate_remote(item.remote)
    if not re.fullmatch(r"[0-9a-f]{64}", item.remote_url_sha256):
        raise StateError(f"{label}.remote_url_sha256 must be a lowercase SHA-256")
    _validate_ref(item.branch, "branch")
    if not SHA_RE.fullmatch(item.start_point):
        raise StateError(f"{label}.start_point must be a full lowercase Git commit")
    _validate_full_ref(item.landed_ref, "landed ref")
    return item


def _owner_from_arg(owner_pid: int | None) -> ProcessIdentity | None:
    return _read_process_identity(owner_pid) if owner_pid is not None else None


def _cmd_init(args: argparse.Namespace) -> int:
    candidate_root = Path(args.directory).absolute()
    if candidate_root.exists() and (
        not candidate_root.is_dir() or candidate_root.is_symlink()
    ):
        raise Refusal(f"project root must be a real directory: {candidate_root}")
    candidate_root.mkdir(parents=True, exist_ok=True)
    root = candidate_root.resolve(strict=True)
    machine = args.machine or _short_hostname()
    _validate_name(machine, "machine")
    remote = _validate_remote(args.default_remote)
    landed_ref = _validate_full_ref(args.default_landed_ref, "default landed ref")
    if not landed_ref.startswith(f"refs/remotes/{remote}/"):
        raise Refusal(
            "default landed ref must name a remote-tracking ref under "
            f"refs/remotes/{remote}/"
        )
    liveness_relative, liveness_command = _relative_inside(
        root, args.liveness_command, "liveness command"
    )
    if (
        not liveness_command.is_file()
        or liveness_command.is_symlink()
        or (
            liveness_command.suffix != ".py"
            and not os.access(liveness_command, os.X_OK)
        )
    ):
        raise Refusal(
            f"liveness command must be a runnable real file: {liveness_command}"
        )
    worktrees_relative, worktrees = _relative_inside(
        root, args.worktrees_dir, "worktrees directory"
    )
    layout = _validate_layout(args.layout)
    if args.heartbeat_ttl_seconds < 1:
        raise Refusal("heartbeat TTL must be at least one second")
    if args.max_active_slots is not None and args.max_active_slots < 0:
        raise Refusal("max active slots must be non-negative")
    cache_globs = tuple(_validate_cache_glob(value) for value in args.cache_glob or ())
    if len(cache_globs) != len(set(cache_globs)):
        raise Refusal("cache globs must not contain duplicates")
    repo_cache_globs = _cache_glob_assignments(
        args.repo_cache_glob or (), "repository cache glob"
    )
    post_provision_hooks = tuple(
        _validate_hook(value) for value in args.post_provision_hook or ()
    )
    disk_gib = (
        args.disk_advisory_gib,
        args.disk_provisioning_floor_gib,
        args.disk_emergency_gib,
    )
    if any(value is not None for value in disk_gib) and not all(
        value is not None for value in disk_gib
    ):
        raise Refusal(
            "disk policy requires --disk-advisory-gib, "
            "--disk-provisioning-floor-gib, and --disk-emergency-gib together"
        )
    if all(value is not None for value in disk_gib):
        advisory_gib, provisioning_gib, emergency_gib = disk_gib
        assert advisory_gib is not None
        assert provisioning_gib is not None
        assert emergency_gib is not None
        if min(advisory_gib, provisioning_gib, emergency_gib) < 1:
            raise Refusal("disk thresholds must each be at least 1 GiB")
        if not emergency_gib < provisioning_gib < advisory_gib:
            raise Refusal(
                "disk thresholds must satisfy emergency < provisioning floor < advisory"
            )
        disk_bytes = (
            advisory_gib * GIB,
            provisioning_gib * GIB,
            emergency_gib * GIB,
        )
    else:
        disk_bytes = (None, None, None)
    payload = _config_payload(
        worktrees_relative,
        machine,
        remote,
        landed_ref,
        args.heartbeat_ttl_seconds,
        liveness_relative,
        args.max_active_slots,
        layout,
        cache_globs,
        repo_cache_globs,
        post_provision_hooks,
        disk_bytes[0],
        disk_bytes[1],
        disk_bytes[2],
    )
    config_path = root / CONFIG_NAME
    with _locked_config(config_path, args.wait_lock):
        _recover_config_write(config_path, payload)
        if config_path.exists() or config_path.is_symlink():
            existing = _as_mapping(_read_json(config_path, "configuration"), "configuration")
            if _canonical_config_payload(existing) != payload:
                existing_worktrees = existing.get("worktrees_dir")
                if isinstance(existing_worktrees, str):
                    _relative, existing_worktrees_path = _relative_inside(
                        root, existing_worktrees, "existing worktrees directory"
                    )
                    existing_layout = existing.get("layout", "nested")
                    # `isinstance` FIRST: a membership test against a set of strings tells mypy
                    # nothing about the type, and this value comes off a decoded record as
                    # `object`. Nothing changes at runtime — a non-string was never in that set.
                    if isinstance(existing_layout, str) and existing_layout in {"nested", "flat"}:
                        existing_control = _control_directory_for(
                            existing_worktrees_path, existing_layout
                        )
                        journals = sorted(existing_control.glob("ACTIVE.*.journal"))
                        if journals:
                            raise Refusal(
                                f"cannot change configuration while recovery journal "
                                f"{journals[0]} exists; recover or explicitly abort it first"
                            )
                if not args.repair:
                    raise Refusal(
                        f"{config_path} already has different configuration; "
                        "do not move live state by rerunning init. If this "
                        "configuration was written by an older build and no "
                        "longer loads, migrate it with `init --repair`."
                    )
                repaired, changes = _repaired_config(dict(existing), payload)
                _atomic_write_json(config_path, repaired)
                for line in changes:
                    print(f"REPAIRED {config_path}: {line}")
            elif args.repair:
                print(f"UNCHANGED {config_path}: already current")
        else:
            _atomic_write_json(config_path, payload)
    if worktrees.exists() and (not worktrees.is_dir() or worktrees.is_symlink()):
        raise Refusal(f"worktrees directory is not a real directory: {worktrees}")
    worktrees.mkdir(parents=True, exist_ok=True)
    config = _load_config(str(root), machine)
    worktrees = config.worktrees
    control = config.control
    with _mutation_locks(config, args.wait_lock):
        _recover_partial_updates(config, True)
        _refuse_partial_state(config)
        if args.repair:
            _repair_empty_state_from_schema_one(config)
        active_path = _active_path(config)
        if active_path.exists() or active_path.is_symlink():
            _load_active(config)
        else:
            _atomic_write_json(active_path, _active_to_obj(ActiveState(machine, 0, ())))
        archive_path = _archive_path(config)
        if archive_path.exists() or archive_path.is_symlink():
            _load_archive(config)
        else:
            _atomic_write_json(
                archive_path, _archive_to_obj(ArchiveState(machine, 0, ()))
            )
        _ensure_event_log(config)
        link = control / "wrkslots"
        executable = Path(__file__).with_name("__main__.py").resolve()
        relative_target = Path(os.path.relpath(executable, start=control))
        if link.exists() or link.is_symlink():
            if not link.is_symlink():
                raise Refusal(f"control path exists and is not a symlink: {link}")
            existing_target = Path(os.readlink(link))
            previous_executable = executable.parent.with_suffix(".py")
            existing_executable = (link.parent / existing_target).resolve()
            if (
                args.repair
                and previous_executable.is_file()
                and not previous_executable.is_symlink()
                and existing_executable == previous_executable
            ):
                _atomic_replace_symlink(link, relative_target)
                print(f"REPAIRED {link}: {existing_target} -> {relative_target}")
            elif existing_target != relative_target:
                raise Refusal(
                    f"control symlink points elsewhere: {link} -> {existing_target}"
                )
        else:
            link.symlink_to(relative_target)
            _fsync_directory(control)
    print(f"initialized wrkslots in {root}")
    print(
        f"machine={machine} worktrees={config.worktrees.relative_to(root)} "
        f"layout={config.layout} "
        f"control={control.relative_to(root)}"
    )
    print(f"command={link} -> {relative_target}")
    return 0


def _create_plan(config: Config, args: argparse.Namespace, vcs: _GitVcs) -> tuple[PlannedCheckout, ...]:
    repositories = _assignments(args.repo, "repository")
    remotes = _assignments(args.remote, "remote")
    remote_urls = _assignments(args.remote_url, "trusted remote URL")
    branches = _assignments(args.branch, "branch")
    starts = _assignments(args.start, "start point")
    if not repositories:
        raise Refusal("create requires at least one --repo NAME=PATH")
    if config.layout == "flat" and len(repositories) != 1:
        raise Refusal("flat layout requires exactly one repository per slot")
    if args.slot_type == "validate" and not branches:
        branches = {
            name: f"wrkslots/validate/{config.machine}/{args.slot}/{name}"
            for name in repositories
        }
    if set(branches) != set(repositories):
        missing = sorted(set(repositories) - set(branches))
        extra = sorted(set(branches) - set(repositories))
        raise Refusal(
            "branches must exactly match repositories"
            + (f"; missing {', '.join(missing)}" if missing else "")
            + (f"; unknown {', '.join(extra)}" if extra else "")
        )
    for mapping, label in (
        (starts, "start point"),
        (remotes, "remote"),
        (remote_urls, "trusted remote URL"),
    ):
        unknown = sorted(set(mapping) - set(repositories))
        if unknown:
            raise Refusal(f"{label} supplied for unknown repository: {', '.join(unknown)}")
    planned: list[PlannedCheckout] = []
    common_directories: set[Path] = set()
    for name, raw_repository in repositories.items():
        repository_relative, repository = _repository_path(config, raw_repository)
        if vcs.repository_root(repository) != repository.absolute():
            raise Refusal(f"repository path is not its Git worktree root: {repository}")
        common = vcs.common_directory(repository)
        if common in common_directories:
            raise Refusal("the same Git repository was supplied more than once")
        common_directories.add(common)
        branch = branches[name]
        vcs.check_branch_name(repository, branch)
        if vcs.branch_exists(repository, branch):
            raise Refusal(f"local branch already exists: {branch}")
        remote = _validate_remote(remotes.get(name, config.default_remote))
        landed_ref = _landed_ref_for_remote(config, remote)
        if not landed_ref.startswith(f"refs/remotes/{remote}/"):
            raise Refusal(
                f"landed ref for {name} must be under refs/remotes/{remote}/"
            )
        authorized_url = remote_urls.get(name)
        remote_url_sha256 = (
            vcs.remote_url_sha256(repository, remote)
            if authorized_url is None
            else vcs.assert_remote_url(repository, remote, authorized_url)
        )
        if args.slot_type != "validate" or name not in starts:
            vcs.fetch_remote(repository, remote, landed_ref)
        verified_remote_url_sha256 = (
            vcs.remote_url_sha256(repository, remote)
            if authorized_url is None
            else vcs.assert_remote_url(repository, remote, authorized_url)
        )
        if verified_remote_url_sha256 != remote_url_sha256:
            raise Refusal(f"remote {remote!r} URL changed during fetch")
        start = vcs.verify_ref(
            repository, starts.get(name, landed_ref), "start point"
        )
        destination_relative, destination = _checkout_path(
            config, args.slot, name, args.slot_type
        )
        if destination.exists() or destination.is_symlink():
            raise Refusal(f"checkout destination already exists: {destination}")
        planned.append(
            PlannedCheckout(
                name=name,
                destination=destination_relative,
                repository=repository_relative,
                branch=branch,
                start_point=start,
                remote=remote,
                remote_url_sha256=remote_url_sha256,
                landed_ref=landed_ref,
            )
        )
    return tuple(sorted(planned, key=lambda item: item.name))


def _assert_active_slot_cap(config: Config, states: Sequence[ActiveState]) -> None:
    """Refuse a new allocation once this machine is at its configured cap.

    The cap counts rows in this machine's shard, which is the same population
    the allocation is about to join. It is enforced here, on the allocation
    path, rather than reported after the fact: a cap that only appears in a
    report is not a cap, and a breach discovered later cannot un-allocate the
    slot that caused it.
    """
    cap = config.max_active_slots
    if cap is None:
        return
    active = [
        record
        for state in states
        if state.machine == config.machine
        for record in state.slots
    ]
    if len(active) < cap:
        return
    raise Refusal(
        f"machine {config.machine} already holds {len(active)} active slot(s) "
        f"against max_active_slots={cap}; finish or remove a slot before "
        f"allocating another. Active: {', '.join(sorted(r.slot for r in active))}"
    )


def _free_bytes(path: Path) -> int:
    try:
        filesystem = os.statvfs(path)
    except OSError as exc:
        raise Refusal(f"cannot measure free space on {path}: {exc}") from exc
    return filesystem.f_bavail * filesystem.f_frsize


def _disk_status(config: Config) -> dict[str, object]:
    free = _free_bytes(_managed_worktrees_root(config))
    advisory = config.disk_advisory_bytes
    provisioning = config.disk_provisioning_floor_bytes
    emergency = config.disk_emergency_bytes
    if advisory is None or provisioning is None or emergency is None:
        state = "unconfigured"
    elif free < emergency:
        state = "emergency"
    elif free < provisioning:
        state = "provisioning-blocked"
    elif free < advisory:
        state = "advisory"
    else:
        state = "healthy"
    return {
        "free_bytes": free,
        "state": state,
        "advisory_bytes": advisory,
        "provisioning_floor_bytes": provisioning,
        "emergency_bytes": emergency,
    }


def _format_gib(value: int) -> str:
    return f"{value / GIB:.1f} GiB"


def _assert_create_disk_space(config: Config, override: bool) -> None:
    status = _disk_status(config)
    state = _as_str(status["state"], "disk state")
    free = _as_int(status["free_bytes"], "disk free bytes")
    remedy = "run 'wrkslots audit' and 'wrkslots clean-caches', then retry"
    if state == "emergency":
        threshold = _as_int(status["emergency_bytes"], "disk emergency bytes", minimum=1)
        raise Refusal(
            f"free space {_format_gib(free)} is below the emergency floor "
            f"{_format_gib(threshold)}; stop builds, publish durable work, and {remedy}. "
            "--override-disk-floor does not bypass the emergency floor"
        )
    if state == "provisioning-blocked" and not override:
        threshold = _as_int(
            status["provisioning_floor_bytes"],
            "disk provisioning floor bytes",
            minimum=1,
        )
        raise Refusal(
            f"free space {_format_gib(free)} is below the provisioning floor "
            f"{_format_gib(threshold)}; {remedy}. Use --override-disk-floor only "
            "after inspecting the pressure"
        )
    if state == "provisioning-blocked":
        print(
            f"WARNING: overriding disk provisioning floor with {_format_gib(free)} free; "
            f"{remedy}",
            file=sys.stderr,
        )
    elif state == "advisory":
        print(
            f"WARNING: disk free space is advisory at {_format_gib(free)}; {remedy}",
            file=sys.stderr,
        )


def _assert_agent_and_slot_free(
    config: Config,
    slot: str,
    agent: str,
    slot_type: str,
    *,
    enforce_cap: bool = True,
) -> None:
    states = _load_all_active(config)
    for state in states:
        for record in state.slots:
            if record.slot == slot:
                raise Refusal(f"slot {slot!r} is already active on {state.machine}")
            if (
                slot_type == "agent"
                and record.slot_type == "agent"
                and record.import_source is None
                and record.agent == agent
            ):
                raise Refusal(
                    f"agent {agent!r} already owns slot {record.slot!r} on {state.machine}"
                )
    if enforce_cap:
        _assert_active_slot_cap(config, states)
    for archive in _load_all_archives(config):
        for archived_record in archive.records:
            if archived_record.get("slot") == slot:
                raise Refusal(
                    f"slot {slot!r} is archived on {archive.machine}; finished slots are never reused"
                )


def _create_journal_payload(
    config: Config,
    args: argparse.Namespace,
    owner: ProcessIdentity | None,
    coordinator_lease: ProcessIdentity,
    plan: Sequence[PlannedCheckout],
    created: Sequence[Checkout],
    *,
    created_at: str,
    heartbeat_at: str,
    hook_progress: int,
) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "kind": "create",
        "machine": config.machine,
        "slot": args.slot,
        "agent": args.agent,
        "task": args.task,
        "purpose": args.purpose,
        "slot_type": args.slot_type,
        "created_at": created_at,
        "heartbeat_at": heartbeat_at,
        "heartbeat_ttl_seconds": config.heartbeat_ttl_seconds,
        "owner": _identity_to_obj(owner),
        "coordinator_lease": _identity_to_obj(coordinator_lease),
        "planned": [_planned_to_obj(item) for item in plan],
        "created": [_checkout_to_obj(item) for item in created],
        "post_provision_hooks": list(config.post_provision_hooks),
        "hook_progress": hook_progress,
        "hook_failure": None,
        "failure_policy": "leave-for-inspection",
    }


def _run_post_provision_hooks(
    config: Config,
    created: Sequence[Checkout],
    journal_path: Path,
    journal: dict[str, object],
    *,
    start: int = 0,
) -> int:
    hooks = config.post_provision_hooks
    total = len(created) * len(hooks)
    if start < 0 or start > total:
        raise StateError("create journal hook progress is out of range")
    for step in range(start, total):
        checkout = created[step // len(hooks)]
        hook_index = step % len(hooks)
        command = hooks[hook_index]
        path = _stored_path(config, checkout.path, "hook checkout path")
        attempt = {
            "checkout": checkout.name,
            "hook_index": hook_index,
            "command": command,
            "status": "running",
        }
        journal["hook_failure"] = attempt
        _write_journal(config, journal)
        env = os.environ.copy()
        env.update(
            {
                "WRKSLOTS_PROJECT_ROOT": str(config.root),
                "WRKSLOTS_SLOT": path.parent.name if config.layout == "nested" else path.name,
                "WRKSLOTS_CHECKOUT": str(path),
                "WRKSLOTS_CHECKOUT_NAME": checkout.name,
            }
        )
        print(
            f"post-provision hook {step + 1}/{total} checkout={checkout.name}: {command}",
            file=sys.stderr,
        )
        try:
            completed = subprocess.run(
                ["/bin/sh", "-c", command],
                cwd=path,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
        except OSError as exc:
            attempt["status"] = "failed-to-start"
            attempt["detail"] = str(exc)
            journal["hook_failure"] = attempt
            _write_journal(config, journal)
            raise Refusal(
                f"post-provision hook {hook_index + 1} for checkout {checkout.name} "
                f"could not start: {exc}; slot was left for inspection with journal "
                f"{journal_path}. Fix the cause and run 'wrkslots recover'"
            ) from exc
        if completed.returncode != 0:
            attempt["status"] = "failed"
            attempt["returncode"] = completed.returncode
            journal["hook_failure"] = attempt
            _write_journal(config, journal)
            if completed.stdout:
                print("post-provision hook stdout:", file=sys.stderr)
                print(completed.stdout, end="" if completed.stdout.endswith("\n") else "\n", file=sys.stderr)
            if completed.stderr:
                print("post-provision hook stderr:", file=sys.stderr)
                print(completed.stderr, end="" if completed.stderr.endswith("\n") else "\n", file=sys.stderr)
            raise Refusal(
                f"post-provision hook {hook_index + 1} for checkout {checkout.name} "
                f"failed with rc {completed.returncode}; slot was left for inspection "
                f"with journal {journal_path}. Fix the cause and run 'wrkslots recover'"
            )
        journal["hook_progress"] = step + 1
        journal["hook_failure"] = None
        _write_journal(config, journal)
    return total


def _cmd_create(args: argparse.Namespace) -> int:
    _require_coordinator_authorized(args, "worktree creation")
    args.slot_type = _require_slot_type(args, "worktree creation")
    config = _load_config(args.project_root, args.machine)
    _validate_name(args.slot, "slot")
    _validate_name(args.agent, "agent")
    if not args.task or not args.purpose:
        raise Refusal("task and purpose must be non-empty")
    owner = (
        _capture_caller_process(args.owner_pid, "owner")
        if args.owner_pid is not None
        else None
    )
    coordinator_lease = _capture_caller_process(
        args.coordinator_pid, "coordinator"
    )
    vcs = _GitVcs()
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        before = _global_rows(states, archives)
        _ensure_state_shard(config)
        state = _load_active(config)
        _assert_agent_and_slot_free(config, args.slot, args.agent, args.slot_type)
        _assert_create_disk_space(config, args.override_disk_floor)
        slot_path = _slot_directory(config, args.slot, args.slot_type)
        slot_path.parent.mkdir(parents=True, exist_ok=True)
        if slot_path.exists() or slot_path.is_symlink():
            raise Refusal(f"slot path already exists: {slot_path}")
        plan = _create_plan(config, args, vcs)
        created_at = _utc_now()
        heartbeat_at = created_at
        journal_path = _journal_path(config)
        created: list[Checkout] = []
        _write_journal(
            config,
            _create_journal_payload(
                config,
                args,
                owner,
                coordinator_lease,
                plan,
                created,
                created_at=created_at,
                heartbeat_at=heartbeat_at,
                hook_progress=0,
            ),
        )
        if config.layout == "nested":
            slot_path.mkdir(mode=0o755)
            _fsync_directory(slot_path.parent)
        for item in plan:
            _relative, repository = _stored_repository_path(config, item.repository)
            destination = _stored_path(config, item.destination, "checkout destination")
            head = vcs.add_worktree(repository, destination, item.branch, item.start_point)
            checkout = Checkout(
                name=item.name,
                path=item.destination,
                repository=item.repository,
                branch=item.branch,
                start_point=item.start_point,
                remote=item.remote,
                remote_url_sha256=item.remote_url_sha256,
                landed_ref=item.landed_ref,
                head=head,
            )
            created.append(checkout)
            _write_journal(
                config,
                _create_journal_payload(
                    config,
                    args,
                    owner,
                    coordinator_lease,
                    plan,
                    created,
                    created_at=created_at,
                    heartbeat_at=heartbeat_at,
                    hook_progress=0,
                ),
            )
            _interrupt_for_test("after-create-worktree")
        hook_journal = _create_journal_payload(
            config,
            args,
            owner,
            coordinator_lease,
            plan,
            created,
            created_at=created_at,
            heartbeat_at=heartbeat_at,
            hook_progress=0,
        )
        hook_progress = _run_post_provision_hooks(
            config, created, journal_path, hook_journal
        )
        for checkout in created:
            _assert_checkout_identity_unchanged(config, checkout, vcs)
        if owner is not None:
            try:
                confirmed_owner = _read_process_identity(owner.pid)
            except Refusal as exc:
                raise Refusal(
                    f"owner process changed during create; recovery journal is preserved: {exc}"
                ) from exc
            if confirmed_owner != owner:
                raise Refusal(
                    "owner process generation changed during create; "
                    "recovery journal is preserved"
                )
            _assert_caller_process(confirmed_owner, "owner")
        confirmed_coordinator = _read_process_identity(coordinator_lease.pid)
        if confirmed_coordinator != coordinator_lease:
            raise Refusal(
                "coordinator process generation changed during create; recovery journal is preserved"
            )
        _assert_caller_process(confirmed_coordinator, "coordinator")
        heartbeat_at = _utc_now()
        _write_journal(
            config,
            _create_journal_payload(
                config,
                args,
                owner,
                coordinator_lease,
                plan,
                created,
                created_at=created_at,
                heartbeat_at=heartbeat_at,
                hook_progress=hook_progress,
            ),
        )
        record = ActiveRecord(
            slot=args.slot,
            agent=args.agent,
            task=args.task,
            purpose=args.purpose,
            slot_type=args.slot_type,
            machine=config.machine,
            generation=1,
            created_at=created_at,
            heartbeat_at=heartbeat_at,
            heartbeat_ttl_seconds=config.heartbeat_ttl_seconds,
            owner=owner,
            coordinator_lease=coordinator_lease,
            coordinator_recovery_note=None,
            handoff=None,
            checkouts=tuple(created),
        )
        state = _append_record(state, record)
        _write_active_state(
            config,
            state,
            action="slot-created",
            slot=record.slot,
            evidence={
                "slot_type": record.slot_type,
                "coordinator_authorized": True,
                "coordinator": _identity_to_obj(coordinator_lease),
            },
        )
        _interrupt_for_test("after-active-write")
        _clear_journal(
            config,
            _as_mapping(_read_json(journal_path, "create journal"), "create journal"),
        )
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    if args.format == "json":
        print(
            json.dumps(
                {
                    "slot": record.slot,
                    "slot_type": record.slot_type,
                    "agent": record.agent,
                    "generation": record.generation,
                    "owner_process": "unbound" if owner is None else "bound",
                    "checkouts": [
                        {
                            "name": checkout.name,
                            "path": str(
                                _stored_path(config, checkout.path, "checkout path")
                            ),
                            "head": checkout.head,
                        }
                        for checkout in record.checkouts
                    ],
                },
                indent=2,
                sort_keys=True,
            )
        )
    else:
        print(
            f"created slot={record.slot} agent={record.agent} generation={record.generation} "
            f"checkouts={len(record.checkouts)}"
        )
        for checkout in record.checkouts:
            print(
                f"checkout={checkout.name} "
                f"path={_stored_path(config, checkout.path, 'checkout path')} "
                f"head={checkout.head}"
            )
        if owner is None:
            print("owner_process=unbound; run 'wrkslots adopt' before heartbeat or finish")
    return 0


def _historical_source_row(
    config: Config, args: argparse.Namespace
) -> tuple[_ImportSource, Mapping[str, object], ProcessIdentity, str, str, str, str]:
    source_relative, source_path = _relative_inside(
        config.root, args.from_state_file, "historical state file"
    )
    _refuse_symlink(source_path, "historical state file")
    try:
        source_bytes = source_path.read_bytes()
        source_value: object = json.loads(source_bytes.decode("utf-8"))
    except FileNotFoundError as exc:
        raise Refusal(
            f"historical state file is missing: {source_path}. state: REFUSED -- "
            "no import journal or active row was written. remedy: restore the exact "
            "source file and rerun import-existing"
        ) from exc
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise Refusal(
            f"cannot read historical state file {source_path}: {exc}. state: REFUSED -- "
            "no import journal or active row was written. remedy: preserve and repair "
            "the source file, then rerun import-existing"
        ) from exc
    source = _as_mapping(source_value, "historical state file")
    _exact_keys(
        source,
        {"version", "updated", "slots", "lease_history"},
        set(),
        "historical state file",
    )
    if _as_int(source["version"], "historical state file.version") != 3:
        raise Refusal(
            "historical state file has an unsupported version. state: REFUSED -- "
            "no import journal or active row was written. remedy: use a version 3 "
            "worktree-state.json source or add a separately reviewed converter"
        )
    _parse_timestamp(
        _as_str(source["updated"], "historical state file.updated"),
        "historical state file.updated",
    )
    slots = _as_mapping(source["slots"], "historical state file.slots")
    if args.slot not in slots:
        raise Refusal(
            f"historical state file has no row for slot {args.slot}. state: REFUSED -- "
            "no import journal or active row was written. remedy: select a slot named "
            "by the source file rather than reconstructing one from disk"
        )
    row = _as_mapping(slots[args.slot], f"historical slot {args.slot}")
    status = _as_str(row.get("status"), f"historical slot {args.slot}.status")
    if status == "released":
        raise Refusal(
            f"historical slot {args.slot} is recorded released. state: REFUSED -- "
            "no active row was written because import-existing only admits storage "
            "that still exists. remedy: retain this source row as history; if its path "
            "exists, investigate the registry disagreement before importing anything"
        )
    if status not in HISTORICAL_ACTIVE_STATUSES:
        raise Refusal(
            f"historical slot {args.slot} has unsupported status {status!r}. state: "
            "REFUSED -- no import journal or active row was written. remedy: preserve "
            "the row and add explicit handling for that status before retrying"
        )
    sidecar_value = row.get("owner_sidecar")
    if sidecar_value is None:
        raise Refusal(
            f"historical slot {args.slot} has no owner_sidecar. state: REFUSED -- no "
            "import journal or active row was written because owner death would remain "
            "unknowable. remedy: leave the slot unregistered and include it in the "
            "migration remainder unless a source with exact PID, start ticks, boot ID, "
            "and cgroup is found"
        )
    sidecar = _as_mapping(sidecar_value, f"historical slot {args.slot}.owner_sidecar")
    _exact_keys(
        sidecar,
        {
            "schema_version",
            "slot",
            "agent",
            "task",
            "recorded_at",
            "source",
            "supervisor_pid",
            "boot_id",
            "start_ticks",
            "cgroup_path",
            "generation",
        },
        {"tmux_pane_id", "lease"},
        f"historical slot {args.slot}.owner_sidecar",
    )
    if _as_int(sidecar["schema_version"], "historical owner schema") != 1:
        raise Refusal(
            f"historical slot {args.slot} has an unsupported owner_sidecar schema. "
            "state: REFUSED -- no import journal or active row was written. remedy: "
            "preserve the row and add explicit handling for that schema before retrying"
        )
    _parse_timestamp(
        _as_str(sidecar["recorded_at"], "historical owner recorded_at"),
        "historical owner recorded_at",
    )
    if not _as_str(sidecar["source"], "historical owner source"):
        raise Refusal(
            f"historical slot {args.slot} owner_sidecar has no source. state: REFUSED -- "
            "no import journal or active row was written. remedy: preserve and repair "
            "the source row before retrying"
        )
    sidecar_slot = _as_str(sidecar["slot"], "historical owner slot")
    if sidecar_slot != args.slot:
        raise Refusal(
            f"historical owner_sidecar names slot {sidecar_slot}, not {args.slot}. state: "
            "REFUSED -- no import journal or active row was written. remedy: repair the "
            "source inconsistency before retrying"
        )
    if args.source_host_id is None:
        raise Refusal(
            "historical owner_sidecar has no stable host identity. state: REFUSED -- no "
            "import journal or active row was written. remedy: on the host that produced "
            "the source registry, pass --source-host-id with its /etc/machine-id value"
        )
    current_host_id = _host_id()
    if args.source_host_id != current_host_id:
        raise Refusal(
            f"--source-host-id does not match this host. state: REFUSED -- no import "
            "journal or active row was written because a foreign process record cannot "
            "prove local owner death. remedy: run the import on the source host or leave "
            "the row in the migration remainder"
        )
    owner = _identity_from_obj(
        {
            "pid": sidecar["supervisor_pid"],
            "start_ticks": sidecar["start_ticks"],
            "boot_id": sidecar["boot_id"],
            "host_id": current_host_id,
            "cgroup_path": sidecar["cgroup_path"],
        },
        f"historical slot {args.slot}.owner",
    )
    assert owner is not None
    owner_state, owner_detail = _process_state(owner)
    if owner_state == "live":
        raise Refusal(
            f"historical slot {args.slot} still has its recorded owner alive: "
            f"{owner_detail}. state: REFUSED -- no import journal or active row was "
            "written. remedy: have that owner use ordinary import-existing with "
            "--verified-live and --owner-pid, or wait for this exact process generation "
            "to exit before retrying the historical import"
        )
    if owner_state != "dead":
        raise Refusal(
            f"historical slot {args.slot} owner death is indeterminate: {owner_detail}. "
            "state: REFUSED -- no import journal or active row was written; unavailable "
            "process evidence is not proof of death. remedy: restore process evidence on "
            "the source host or leave the row in the migration remainder"
        )
    expected_generation = f"{owner.boot_id}:{owner.pid}:{owner.start_ticks}"
    if sidecar.get("generation") is not None and sidecar.get("generation") != expected_generation:
        raise Refusal(
            f"historical slot {args.slot} owner generation disagrees with its process "
            "fields. state: REFUSED -- no import journal or active row was written. "
            "remedy: preserve and repair the source row before retrying"
    )
    agent = _validate_name(_as_str(sidecar["agent"], "historical owner agent"), "agent")
    try:
        task, purpose = _historical_task_and_purpose(
            row, f"historical slot {args.slot}", agent
        )
    except StateError as exc:
        raise Refusal(
            f"historical slot {args.slot} has invalid task or purpose evidence: "
            f"{exc}. state: "
            "REFUSED -- no import journal or active row was written. remedy: repair the "
            "source inconsistency before retrying"
        ) from exc
    created_at = _as_str(row.get("allocated"), f"historical slot {args.slot}.allocated")
    _parse_timestamp(created_at, f"historical slot {args.slot}.allocated")
    import_source = _ImportSource(
        format="worktree-state-v3",
        path=source_relative,
        file_sha256=hashlib.sha256(source_bytes).hexdigest(),
        row_sha256=_event_digest(row),
        status=status,
        row=dict(row),
    )
    return import_source, row, owner, agent, task, purpose, created_at


def _historical_task_and_purpose(
    row: Mapping[str, object], label: str, agent: str
) -> tuple[str, str]:
    sidecar = _as_mapping(row.get("owner_sidecar"), f"{label}.owner_sidecar")
    agents = [
        _as_mapping(item, f"{label}.agents[{index}]")
        for index, item in enumerate(_as_list(row.get("agents"), f"{label}.agents"))
    ]
    writable = [
        item
        for item in agents
        if item.get("name") == agent and item.get("read_only") is False
    ]
    if not writable:
        raise StateError(f"{label} does not list owner {agent} as a writable agent")
    task_values: list[str] = []
    for task_label, value in (
        (f"{label}.task", row.get("task")),
        (f"{label}.owner_sidecar.task", sidecar.get("task")),
    ):
        if value is None:
            task_values.append("")
            continue
        task_values.append(_as_str(value, task_label))
    writable_tasks = {
        _as_str(item.get("task"), f"{label}.writable_agent[{index}].task")
        for index, item in enumerate(writable)
        if item.get("task") is not None
    }
    writable_tasks.discard("")
    if len(writable_tasks) > 1:
        raise StateError(f"{label} records conflicting writable-agent task values")
    row_task, owner_task = task_values
    task = (
        row_task
        or owner_task
        or next(iter(writable_tasks), HISTORICAL_TASK_NOT_RECORDED)
    )
    raw_purpose = row.get("purpose")
    purpose = (
        HISTORICAL_PURPOSE_NOT_RECORDED
        if raw_purpose is None
        else _as_str(raw_purpose, f"{label}.purpose")
    )
    if not purpose:
        purpose = HISTORICAL_PURPOSE_NOT_RECORDED
    return task, purpose


def _historical_checkouts(
    config: Config,
    args: argparse.Namespace,
    row: Mapping[str, object],
    vcs: _GitVcs,
) -> tuple[Checkout, ...]:
    repositories = _assignments(args.repo, "repository")
    remotes = _assignments(args.remote, "remote")
    remote_urls = _assignments(args.remote_url, "trusted remote URL")
    source_paths: dict[str, tuple[str, Path]] = {}
    for key, value in row.items():
        if not key.endswith("_path"):
            continue
        name = _validate_name(key.removesuffix("_path"), "historical checkout name")
        raw_path = _as_str(value, f"historical slot {args.slot}.{key}")
        if raw_path == "-":
            continue
        relative, destination = _relative_inside(
            config.root, raw_path, f"historical checkout {name} path"
        )
        expected, expected_path = _checkout_path_for_layout(
            config, args.slot, name, args.slot_type, "nested"
        )
        if relative != expected or destination != expected_path:
            raise Refusal(
                f"historical checkout {name} path {raw_path!r} is not the nested path "
                f"{expected!r} for slot {args.slot}. state: REFUSED -- no import journal "
                "or active row was written. remedy: repair the source row or move the "
                "checkout through a separately reviewed operation before retrying"
            )
        source_paths[name] = (relative, destination)
    if not source_paths:
        raise Refusal(
            f"historical slot {args.slot} has no NAME_path fields. state: REFUSED -- "
            "no import journal or active row was written. remedy: preserve and repair "
            "the source row before retrying"
        )
    unknown_repositories = sorted(set(repositories) - set(source_paths))
    if unknown_repositories:
        raise Refusal(
            "repository supplied for checkout absent from the historical row: "
            + ", ".join(unknown_repositories)
        )
    for mapping, label in (
        (remotes, "remote"),
        (remote_urls, "trusted remote URL"),
    ):
        unknown = sorted(set(mapping) - set(repositories))
        if unknown:
            raise Refusal(f"{label} supplied for unknown repository: {', '.join(unknown)}")
    present_without_repository = sorted(
        name
        for name, (_relative, destination) in source_paths.items()
        if (destination.exists() or destination.is_symlink()) and name not in repositories
    )
    if present_without_repository:
        rendered = " ".join(
            f"--repo {name}=<source-repository>" for name in present_without_repository
        )
        raise Refusal(
            "historical slot contains checkout paths with no repository mapping: "
            f"{', '.join(present_without_repository)}. state: REFUSED -- no import "
            "journal or active row was written. remedy: rerun with {rendered}"
        )
    checkouts: list[Checkout] = []
    for name, raw_repository in sorted(repositories.items()):
        relative, destination = source_paths[name]
        repository_relative, repository = _repository_path(config, raw_repository)
        if not destination.exists() and not destination.is_symlink():
            if destination.absolute() in vcs.listed_worktrees(repository):
                raise Refusal(
                    f"historical checkout {name} is missing but Git still registers "
                    f"{destination}. state: REFUSED -- no import journal or active row "
                    "was written. remedy: repair the Git worktree registration before retrying"
                )
            continue
        if destination.is_symlink() or not destination.is_dir():
            raise Refusal(
                f"historical checkout is unsafe: {destination}. state: REFUSED -- no "
                "import journal or active row was written. remedy: preserve and inspect "
                "the path before retrying"
            )
        head = vcs.verify_existing_worktree(repository, destination)
        branch = vcs.branch(destination)
        remote = _validate_remote(remotes.get(name, config.default_remote))
        authorized_url = remote_urls.get(name)
        remote_url_sha256 = (
            vcs.remote_url_sha256(destination, remote)
            if authorized_url is None
            else vcs.assert_remote_url(destination, remote, authorized_url)
        )
        checkouts.append(
            Checkout(
                name=name,
                path=relative,
                repository=repository_relative,
                branch=branch,
                start_point=head,
                remote=remote,
                remote_url_sha256=remote_url_sha256,
                landed_ref=_landed_ref_for_remote(config, remote),
                head=head,
            )
        )
    return tuple(checkouts)


def _historical_import_inputs(
    config: Config, args: argparse.Namespace, vcs: _GitVcs
) -> _HistoricalImport:
    if args.slot_type != "agent":
        raise Refusal(
            "--from-state-file describes historical agent slots only. state: REFUSED -- "
            "no import journal or active row was written. remedy: use ordinary "
            "import-existing for a live validate slot"
        )
    if args.owner_pid is not None or args.verified_live:
        raise Refusal(
            "--owner-pid and --verified-live describe current ownership, not a historical "
            "source row. state: REFUSED -- no import journal or active row was written. "
            "remedy: drop them when using --from-state-file"
        )
    if any(value is not None for value in (args.agent, args.task, args.purpose)):
        raise Refusal(
            "--agent, --task, and --purpose are owned by the historical source row. "
            "state: REFUSED -- no import journal or active row was written. remedy: drop "
            "those flags or repair the source row before retrying"
        )
    source, row, owner, agent, task, purpose, created_at = _historical_source_row(
        config, args
    )
    checkouts = _historical_checkouts(config, args, row, vcs)
    try:
        _assert_slot_contents_values(
            config, args.slot, args.slot_type, "nested", checkouts
        )
    except Refusal as exc:
        raise Refusal(
            f"historical slot {args.slot} does not match the source row: {exc}. state: "
            "REFUSED -- no import journal or active row was written. remedy: preserve "
            "the source and slot, reconcile their paths and contents, then rerun the dry run"
        ) from exc
    return _HistoricalImport(
        agent=agent,
        task=task,
        purpose=purpose,
        created_at=created_at,
        owner=owner,
        checkouts=checkouts,
        source=source,
    )


def _existing_checkouts(
    config: Config, args: argparse.Namespace, vcs: _GitVcs
) -> tuple[Checkout, ...]:
    repositories = _assignments(args.repo, "repository")
    remotes = _assignments(args.remote, "remote")
    remote_urls = _assignments(args.remote_url, "trusted remote URL")
    if not repositories:
        raise Refusal("at least one --repo NAME=PATH is required")
    if config.layout == "flat" and len(repositories) != 1:
        raise Refusal("flat layout requires exactly one repository per slot")
    for mapping, label in (
        (remotes, "remote"),
        (remote_urls, "trusted remote URL"),
    ):
        unknown = sorted(set(mapping) - set(repositories))
        if unknown:
            raise Refusal(f"{label} supplied for unknown repository: {', '.join(unknown)}")
    checkouts: list[Checkout] = []
    for name, raw_repository in repositories.items():
        repository_relative, repository = _repository_path(config, raw_repository)
        destination_relative, destination = _checkout_path(
            config, args.slot, name, args.slot_type
        )
        if not destination.is_dir() or destination.is_symlink():
            raise Refusal(f"existing checkout is missing or unsafe: {destination}")
        head = vcs.verify_existing_worktree(repository, destination)
        branch = vcs.branch(destination)
        remote = _validate_remote(remotes.get(name, config.default_remote))
        authorized_url = remote_urls.get(name)
        remote_url_sha256 = (
            vcs.remote_url_sha256(destination, remote)
            if authorized_url is None
            else vcs.assert_remote_url(destination, remote, authorized_url)
        )
        landed_ref = _landed_ref_for_remote(config, remote)
        if not landed_ref.startswith(f"refs/remotes/{remote}/"):
            raise Refusal(
                f"landed ref for {name} must be under refs/remotes/{remote}/"
            )
        checkouts.append(
            Checkout(
                name=name,
                path=destination_relative,
                repository=repository_relative,
                branch=branch,
                start_point=head,
                remote=remote,
                remote_url_sha256=remote_url_sha256,
                landed_ref=landed_ref,
                head=head,
            )
        )
    return tuple(sorted(checkouts, key=lambda item: item.name))


def _register_existing(
    config: Config,
    args: argparse.Namespace,
    owner: ProcessIdentity,
    coordinator_lease: ProcessIdentity,
) -> ActiveRecord:
    _validate_name(args.slot, "slot")
    _validate_name(args.agent, "agent")
    if not args.task or not args.purpose:
        raise Refusal("task and purpose must be non-empty")
    if not args.verified_live:
        raise Refusal("registration requires --verified-live and a live --owner-pid")
    vcs = _GitVcs()
    checkouts = _existing_checkouts(config, args, vcs)
    slot_path = _assert_slot_contents(
        config,
        ActiveRecord(
            slot=args.slot,
            agent=args.agent,
            task=args.task,
            purpose=args.purpose,
            slot_type=args.slot_type,
            machine=config.machine,
            generation=1,
            created_at=_utc_now(),
            heartbeat_at=_utc_now(),
            heartbeat_ttl_seconds=config.heartbeat_ttl_seconds,
            owner=owner,
            coordinator_lease=coordinator_lease,
            coordinator_recovery_note=None,
            handoff=None,
            checkouts=checkouts,
        ),
    )
    for checkout in checkouts:
        _assert_checkout_identity_unchanged(config, checkout, vcs)
    confirmed_owner = _read_process_identity(owner.pid)
    if confirmed_owner != owner:
        raise Refusal("owner process generation changed during registration")
    confirmed_coordinator = _read_process_identity(coordinator_lease.pid)
    if confirmed_coordinator != coordinator_lease:
        raise Refusal("coordinator process generation changed during registration")
    _assert_caller_process(confirmed_coordinator, "coordinator")
    _assert_registration_owner_authorized(
        confirmed_owner, confirmed_coordinator, slot_path
    )
    now = _utc_now()
    return ActiveRecord(
        slot=args.slot,
        agent=args.agent,
        task=args.task,
        purpose=args.purpose,
        slot_type=args.slot_type,
        machine=config.machine,
        generation=1,
        created_at=now,
        heartbeat_at=now,
        heartbeat_ttl_seconds=config.heartbeat_ttl_seconds,
        owner=owner,
        coordinator_lease=coordinator_lease,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=checkouts,
    )


def _cmd_register(
    args: argparse.Namespace, *, allow_unregistered_migration_slots: bool = False
) -> int:
    _require_coordinator_authorized(args, "worktree registration")
    args.slot_type = _require_slot_type(args, "worktree registration")
    config = _load_config(args.project_root, args.machine)
    owner = _capture_registration_owner(args.owner_pid)
    coordinator_lease = _capture_caller_process(
        args.coordinator_pid, "coordinator"
    )
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_command_registry_storage(
            config,
            states,
            args,
            allowed_unregistered_slot=args.slot,
            migration_operation=allow_unregistered_migration_slots,
        )
        before = _global_rows(states, archives)
        _ensure_state_shard(config)
        state = _load_active(config)
        _assert_agent_and_slot_free(config, args.slot, args.agent, args.slot_type)
        record = _register_existing(
            config, args, owner, coordinator_lease
        )
        _write_active_state(
            config,
            _append_record(state, record),
            action="slot-registered",
            slot=record.slot,
            evidence={
                "slot_type": record.slot_type,
                "coordinator_authorized": True,
                "coordinator": _identity_to_obj(coordinator_lease),
            },
        )
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    print(
        f"registered slot={record.slot} agent={record.agent} generation=1 "
        f"checkouts={len(record.checkouts)}"
    )
    return 0


def _import_journal_payload(record: ActiveRecord) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "kind": "import-existing",
        "machine": record.machine,
        "slot": record.slot,
        "record": _record_to_obj(record),
    }


def _verify_import_record(
    config: Config, record: ActiveRecord, vcs: _GitVcs
) -> None:
    _assert_record_paths(config, record)
    _assert_slot_contents(config, record)
    for checkout in record.checkouts:
        _assert_checkout_identity_unchanged(config, checkout, vcs)


def _publish_import(
    config: Config,
    state: ActiveState,
    record: ActiveRecord,
    *,
    coordinator_authorized: bool,
) -> None:
    journal = _import_journal_payload(record)
    _write_journal(config, journal)
    _interrupt_for_test("after-import-journal")
    _verify_import_record(config, record, _GitVcs())
    _write_active_state(
        config,
        _append_record(state, record),
        action="slot-imported",
        slot=record.slot,
        evidence={
            "slot_type": record.slot_type,
            "coordinator_authorized": coordinator_authorized,
            "coordinator": _identity_to_obj(record.coordinator_lease),
            "import_source": _import_source_to_obj(record.import_source),
        },
    )
    _interrupt_for_test("after-import-active-write")
    _clear_journal(config, journal)


def _cmd_import_existing(args: argparse.Namespace) -> int:
    args.slot_type = _require_slot_type(args, "worktree import")
    config = _load_config(args.project_root, args.machine)
    vcs = _GitVcs()
    _validate_name(args.slot, "slot")
    historical = (
        None
        if args.from_state_file is None
        else _historical_import_inputs(config, args, vcs)
    )
    if historical is None:
        if args.source_host_id is not None:
            raise Refusal(
                "--source-host-id applies only with --from-state-file. state: REFUSED -- "
                "no import journal or active row was written. remedy: drop the flag or "
                "supply the historical state file it describes"
            )
        if any(value is None for value in (args.agent, args.task, args.purpose)):
            raise Refusal(
                "live import requires --agent, --task, and --purpose. state: REFUSED -- "
                "no import journal or active row was written. remedy: supply all three, "
                "or use --from-state-file to take them from a historical row"
            )
        assert args.agent is not None
        assert args.task is not None
        assert args.purpose is not None
        checkouts = _existing_checkouts(config, args, vcs)
        agent = args.agent
        task = args.task
        purpose = args.purpose
        source = None
    else:
        checkouts = historical.checkouts
        agent = historical.agent
        task = historical.task
        purpose = historical.purpose
        source = historical.source
    print(
        f"verified existing slot={args.slot} checkouts={len(checkouts)} "
        f"machine={config.machine}"
    )
    if source is not None:
        print(
            f"  source={source.path} status={source.status} "
            f"row_sha256={source.row_sha256}"
        )
    for checkout in checkouts:
        print(
            f"  {checkout.name}: path={checkout.path} branch={checkout.branch} head={checkout.head}"
        )
    if not args.apply:
        if historical is None:
            print(
                "dry-run: no state changed; add --apply --verified-live --owner-pid PID "
                "--coordinator-pid PID --coordinator-authorized to register"
            )
        else:
            print(
                "dry-run: no state changed; add --apply --coordinator-pid PID "
                "--coordinator-authorized to record the exact historical row and begin "
                "a fresh heartbeat time-to-live"
            )
        return 0
    _require_coordinator_authorized(args, "worktree import --apply")
    if args.coordinator_pid is None:
        raise Refusal(
            "--apply requires --coordinator-pid. state: REFUSED -- no import journal "
            "or active row was written. remedy: pass the live invoking coordinator PID"
        )
    if historical is None and (args.owner_pid is None or not args.verified_live):
        raise Refusal(
            "live --apply requires --verified-live and --owner-pid. state: REFUSED -- "
            "no import journal or active row was written. remedy: supply both for the "
            "current owner, or use --from-state-file for a recorded historical owner"
        )
    if historical is None:
        assert args.owner_pid is not None
    coordinator_lease = _capture_caller_process(
        args.coordinator_pid, "coordinator"
    )
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_command_registry_storage(
            config,
            states,
            args,
            allowed_unregistered_slot=args.slot,
            migration_operation=True,
        )
        before = _global_rows(states, archives)
        _ensure_state_shard(config)
        state = _load_active(config)
        if historical is not None:
            current_historical = _historical_import_inputs(config, args, vcs)
            if current_historical != historical:
                raise Refusal(
                    "historical source row or checkout identity changed while import was "
                    "starting. state: REFUSED -- no import journal or active row was "
                    "written. remedy: inspect the new source and rerun the dry run before "
                    "applying it"
                )
            historical = current_historical
        _assert_agent_and_slot_free(
            config, args.slot, agent, args.slot_type, enforce_cap=False
        )
        if historical is None:
            owner = _capture_registration_owner(args.owner_pid)
            record = _register_existing(config, args, owner, coordinator_lease)
        else:
            now = _utc_now()
            record = ActiveRecord(
                slot=args.slot,
                agent=historical.agent,
                task=historical.task,
                purpose=historical.purpose,
                slot_type=args.slot_type,
                machine=config.machine,
                generation=1,
                created_at=historical.created_at,
                heartbeat_at=now,
                heartbeat_ttl_seconds=config.heartbeat_ttl_seconds,
                owner=historical.owner,
                coordinator_lease=coordinator_lease,
                coordinator_recovery_note=None,
                handoff=None,
                checkouts=historical.checkouts,
                layout="nested",
                import_source=historical.source,
            )
            _verify_import_record(config, record, vcs)
        confirmed_coordinator = _read_process_identity(coordinator_lease.pid)
        if confirmed_coordinator != coordinator_lease:
            raise Refusal(
                "coordinator process generation changed during import. state: REFUSED -- "
                "no import journal or active row was written. remedy: rerun with the "
                "current invoking coordinator PID"
            )
        _assert_caller_process(confirmed_coordinator, "coordinator")
        _publish_import(
            config,
            state,
            record,
            coordinator_authorized=True,
        )
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    print(
        f"registered slot={record.slot} agent={record.agent} generation=1 "
        f"checkouts={len(record.checkouts)}"
    )
    return 0


def _expected_generation(record: ActiveRecord, expected: int) -> None:
    if record.generation != expected:
        raise Refusal(
            f"stale generation for slot {record.slot}: expected {expected}, "
            f"current {record.generation}"
        )


def _assert_owner_auth(
    record: ActiveRecord,
    agent: str,
    supplied: ProcessIdentity,
    expected_generation: int,
) -> ProcessIdentity:
    _expected_generation(record, expected_generation)
    if record.agent != agent:
        raise Refusal(
            f"ownership mismatch for slot {record.slot}: expected {record.agent}, got {agent}"
        )
    if record.owner is None:
        raise Refusal(f"slot {record.slot} has no bound owner process; adopt it first")
    if supplied != record.owner:
        state, detail = _process_state(record.owner)
        raise Refusal(
            f"owner process generation mismatch for slot {record.slot}; "
            f"recorded owner is {state}: {detail}"
        )
    _assert_caller_process(supplied, "owner")
    return supplied


def _cmd_adopt(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    _validate_name(args.slot, "slot")
    _validate_name(args.agent, "agent")
    owner = _capture_caller_process(args.owner_pid, "owner")
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        before = _global_rows(states, archives)
        state = _load_active(config)
        record = _find_record(state, args.slot)
        _expected_generation(record, args.expected_generation)
        if record.agent != args.agent:
            raise Refusal(
                f"ownership mismatch for slot {args.slot}: expected {record.agent}, got {args.agent}"
            )
        if record.owner == owner:
            print(
                f"already adopted slot={record.slot} agent={record.agent} "
                f"generation={record.generation}"
            )
            return 0
        if record.owner is not None:
            state_name, detail = _process_state(record.owner)
            raise Refusal(
                f"slot {record.slot} already has a {state_name} historical owner: {detail}; "
                "ordinary adopt cannot replace it"
            )
        confirmed_owner = _read_process_identity(owner.pid)
        if confirmed_owner != owner:
            raise Refusal("owner process generation changed during adopt")
        _assert_caller_process(confirmed_owner, "owner")
        now = _utc_now()
        adopted = dataclasses.replace(
            record,
            owner=owner,
            heartbeat_at=now,
        )
        _write_active_state(
            config,
            _replace_record(state, adopted),
            action="owner-bound",
            slot=adopted.slot,
            evidence={"owner": _identity_to_obj(owner)},
        )
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    print(
        f"adopted slot={adopted.slot} agent={adopted.agent} generation={adopted.generation} "
        f"owner_pid={owner.pid}"
    )
    return 0


def _cmd_recover_unbound_owner(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    _validate_name(args.slot, "slot")
    if not args.recovery_note.strip():
        raise Refusal("coordinator recovery requires a non-empty --recovery-note")
    validation = tuple(args.validation or ())
    limitations = tuple(args.limitation or ())
    if not validation or any(not item.strip() for item in validation):
        raise Refusal("coordinator recovery requires non-empty --validation evidence")
    coordinator = _capture_caller_process(args.coordinator_pid, "coordinator")
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        before = _global_rows(states, archives)
        state = _load_active(config)
        record = _find_record(state, args.slot)
        _expected_generation(record, args.expected_generation)
        _assert_caller_process(coordinator, "coordinator")
        if record.owner is not None:
            raise Refusal(
                f"slot {record.slot} has a historical owner; coordinator recovery cannot replace it"
            )
        _assert_registered_liveness(config, record)
        _slot_path, final_checkouts = _handoff_preconditions(
            config, record, _GitVcs()
        )
        continuation = (
            f"wrkslots remove {record.slot} --coordinator-pid "
            f"<current-coordinator-pid> --expected-generation {record.generation}"
        )
        updated = dataclasses.replace(
            record,
            checkouts=final_checkouts,
            coordinator_recovery_note=args.recovery_note,
            handoff=Handoff(
                recorded_at=_utc_now(),
                validation=validation,
                limitations=limitations,
                continuation=continuation,
            ),
        )
        _write_active_state(
            config,
            _replace_record(state, updated),
            action="owner-recovery-recorded",
            slot=updated.slot,
            evidence={"note": args.recovery_note},
        )
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    print(
        f"recorded coordinator recovery evidence slot={updated.slot} "
        f"generation={updated.generation}"
    )
    return 0


def _cmd_heartbeat(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    _validate_name(args.slot, "slot")
    _validate_name(args.agent, "agent")
    owner = _capture_caller_process(args.owner_pid, "owner")
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        before = _global_rows(states, archives)
        state = _load_active(config)
        record = _find_record(state, args.slot)
        _assert_owner_auth(record, args.agent, owner, args.expected_generation)
        updated = dataclasses.replace(record, heartbeat_at=_utc_now())
        _write_active_state(
            config,
            _replace_record(state, updated),
            action="ttl-renewed",
            slot=updated.slot,
            evidence={"heartbeat_at": updated.heartbeat_at},
        )
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    print(
        f"heartbeat slot={updated.slot} generation={updated.generation} at={updated.heartbeat_at}"
    )
    return 0


def _cmd_hold(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    _validate_name(args.slot, "slot")
    reason = args.reason.strip()
    if not reason:
        raise Refusal("hold requires a non-empty --reason")
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, _archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        record = _find_record(_load_active(config), args.slot)
        path = _hold_path(config, record.slot, record.machine)
        existing = _load_hold(config, record.slot, record.machine)
        if existing is not None:
            if _as_str(existing["reason"], "slot hold.reason") != reason:
                raise Refusal(
                    f"slot {record.slot} is already held for a different reason: "
                    f"{existing['reason']}"
                )
            print(f"already held slot={record.slot} reason={reason}")
            return 0
        _ensure_event_log(config, record.machine)
        _write_event_file(
            config,
            record.machine,
            "slot-held",
            {"slot": record.slot, "generation": record.generation, "reason": reason},
        )
        _atomic_write_json(
            path,
            {
                "schema": HOLD_SCHEMA,
                "machine": record.machine,
                "slot": record.slot,
                "held_at": _utc_now(),
                "reason": reason,
            },
        )
    print(f"held slot={record.slot} reason={reason}")
    return 0


def _cmd_unhold(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    _validate_name(args.slot, "slot")
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, _archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        record = _find_record(_load_active(config), args.slot)
        path = _hold_path(config, record.slot, record.machine)
        if _load_hold(config, record.slot, record.machine) is None:
            raise Refusal(f"slot {record.slot} is not held")
        _ensure_event_log(config, record.machine)
        _write_event_file(
            config,
            record.machine,
            "slot-hold-released",
            {"slot": record.slot, "generation": record.generation},
        )
        _remove_control_file(path)
    print(f"released hold slot={record.slot}")
    return 0


def _cmd_read_handoff(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    _validate_name(args.slot, "slot")
    reader = _capture_caller_process(args.coordinator_pid, "coordinator")
    _assert_caller_process(reader, "coordinator")
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, _archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        record = _find_record(_load_active(config), args.slot)
        if record.slot_type != "agent":
            raise Refusal(
                f"slot {record.slot} has type validate and cannot carry an agent handoff. "
                "state: REFUSED -- no event was recorded. remedy: inspect its run evidence "
                "under the project's validation evidence directory"
            )
        handoff = _slot_directory(config, record.slot, record.slot_type) / "HANDOFF.md"
        if not handoff.exists() and not handoff.is_symlink():
            raise Refusal(
                f"slot {record.slot} has no HANDOFF.md. state: REFUSED -- no read was "
                "recorded. remedy: continue the reclaim checks; absence of a handoff is not "
                "evidence that the owner is dead"
            )
        if handoff.is_symlink() or not handoff.is_file():
            raise Refusal(
                f"slot {record.slot} HANDOFF.md is not a regular file. state: REFUSED -- "
                "no read was recorded. remedy: preserve the slot and replace the unsafe path "
                "with the original handoff file before retrying"
            )
        try:
            contents = handoff.read_bytes()
            rendered = contents.decode("utf-8")
        except (OSError, UnicodeError) as exc:
            raise Refusal(
                f"slot {record.slot} HANDOFF.md cannot be read as UTF-8: {exc}. state: "
                "REFUSED -- no read was recorded. remedy: preserve and repair the handoff file"
            ) from exc
        print(f"--- {handoff} ---")
        print(rendered, end="" if rendered.endswith("\n") else "\n")
        print(f"--- end {handoff} ---")
        digest = hashlib.sha256(contents).hexdigest()
        _ensure_event_log(config, record.machine)
        _write_event_file(
            config,
            record.machine,
            "handoff-read",
            {
                "slot": record.slot,
                "generation": record.generation,
                "sha256": digest,
                "contents_utf8": rendered,
                "reader": _identity_to_obj(reader),
                "coordinator_authorized": bool(args.coordinator_authorized),
            },
        )
    print(f"recorded HANDOFF.md read slot={record.slot} sha256={digest}")
    return 0


def _cache_directories_for_path(
    config: Config,
    checkout_path: Path,
    checkout_name: str,
    vcs: _GitVcs,
    cache_globs: Sequence[str],
    source_repository: Path | None = None,
) -> tuple[CacheDirectory, ...]:
    candidates: set[Path] = set()
    if not checkout_path.is_dir() or checkout_path.is_symlink():
        raise Refusal(f"checkout is missing or unsafe for cache inspection: {checkout_path}")
    checkout_identity = _open_directory_identity(checkout_path, "checkout")
    if source_repository is None:
        if vcs.repository_root(checkout_path) != checkout_path.absolute():
            raise Refusal(f"cache target is not a Git checkout root: {checkout_path}")
    else:
        vcs.verify_existing_worktree(source_repository, checkout_path)
    if checkout_identity != _open_directory_identity(checkout_path, "checkout"):
        raise Refusal(f"checkout changed during cache inspection: {checkout_path}")
    _assert_cache_policy_untracked_path(
        config, checkout_name, checkout_path, vcs, cache_globs
    )
    for pattern in cache_globs:
        try:
            matches = checkout_path.glob(pattern)
            for candidate in matches:
                absolute = candidate.absolute()
                if not _path_is_within(absolute, checkout_path) or absolute == checkout_path:
                    raise Refusal(
                        f"cache glob {pattern!r} escaped checkout {checkout_path}"
                    )
                _ensure_no_symlink_components(checkout_path, absolute, "cache path")
                _ensure_no_mount_components(checkout_path, absolute, "cache path")
                _assert_no_mountinfo_crossing(checkout_path, absolute, "cache path")
                if candidate.is_symlink():
                    raise Refusal(f"configured cache path is a symlink: {candidate}")
                if not candidate.exists():
                    continue
                if not candidate.is_dir():
                    raise Refusal(f"configured cache path is not a directory: {candidate}")
                candidates.add(absolute)
        except (OSError, ValueError) as exc:
            raise Refusal(
                f"cannot expand cache glob {pattern!r} in {checkout_path}: {exc}"
            ) from exc
    selected: list[Path] = []
    for candidate in sorted(candidates, key=lambda value: (len(value.parts), str(value))):
        if any(_path_is_within(candidate, parent) for parent in selected):
            continue
        selected.append(candidate)
    if checkout_identity != _open_directory_identity(checkout_path, "checkout"):
        raise Refusal(f"checkout changed during cache inspection: {checkout_path}")
    return tuple(
        CacheDirectory(
            path=path,
            checkout_root=checkout_path,
            checkout_device=checkout_identity[0],
            checkout_inode=checkout_identity[1],
            checkout_mount_id=checkout_identity[2],
        )
        for path in selected
    )


def _cache_directories_for_checkout(
    config: Config, checkout: Checkout
) -> tuple[CacheDirectory, ...]:
    checkout_path = _stored_path(config, checkout.path, "checkout path")
    vcs = _GitVcs()
    _relative, repository = _stored_repository_path(config, checkout.repository)
    return _cache_directories_for_path(
        config,
        checkout_path,
        checkout.name,
        vcs,
        _cache_globs_for(config, checkout.name),
        repository,
    )


def _unregistered_cache_roots(config: Config, slot: str) -> tuple[Path, ...]:
    slot_path = config.worktrees / slot
    if not slot_path.is_dir() or slot_path.is_symlink():
        raise Refusal(f"unregistered slot is missing or unsafe: {slot_path}")
    candidates = (slot_path,) if config.layout == "flat" else tuple(slot_path.iterdir())
    roots: list[Path] = []
    vcs = _GitVcs()
    for candidate in candidates:
        if not candidate.is_dir() or candidate.is_symlink():
            raise Refusal(
                f"unregistered slot {slot} contains a non-checkout entry: {candidate}"
            )
        if vcs.repository_root(candidate) != candidate.absolute():
            raise Refusal(
                f"unregistered slot {slot} entry is not a Git checkout root: {candidate}"
            )
        roots.append(candidate.absolute())
    if not roots:
        raise Refusal(f"unregistered slot {slot} contains no Git checkouts")
    return tuple(roots)


def _cache_directories(
    config: Config, checkouts: Sequence[Checkout]
) -> tuple[CacheDirectory, ...]:
    return tuple(
        path
        for checkout in checkouts
        for path in _cache_directories_for_checkout(config, checkout)
    )


def _cache_slot_directories(
    config: Config, cache_slot: CacheSlot
) -> tuple[CacheDirectory, ...]:
    registered = _cache_directories(config, cache_slot.checkouts)
    vcs = _GitVcs()
    roots = (
        _unregistered_cache_roots(config, cache_slot.slot)
        if cache_slot.state == "unregistered"
        else ()
    )
    if roots and config.repo_cache_globs and not config.cache_globs:
        raise Refusal(
            f"unregistered slot {cache_slot.slot} has no trusted repository-name mapping; "
            "import it or configure a global cache_globs policy before cleanup"
        )
    unregistered: list[CacheDirectory] = []
    for index, root in enumerate(roots):
        label = f"unregistered-{index + 1}"
        unregistered.extend(
            _cache_directories_for_path(
                config,
                root,
                label,
                vcs,
                config.cache_globs,
            )
        )
    return (*registered, *unregistered)


def _journal_cache_slot(config: Config, machine: str | None = None) -> CacheSlot | None:
    selected_machine = machine or config.machine
    path = _journal_path(config, selected_machine)
    if not path.exists() and not path.is_symlink():
        return None
    raw = _as_mapping(_read_json(path, "cache recovery journal"), "cache recovery journal")
    if _as_int(raw.get("schema"), "cache recovery journal.schema") != SCHEMA:
        raise StateError("unsupported cache recovery journal schema")
    if (
        _as_str(raw.get("machine"), "cache recovery journal.machine")
        != selected_machine
    ):
        raise StateError("cache recovery journal belongs to a different machine")
    kind = _as_str(raw.get("kind"), "cache recovery journal.kind")
    slot = _validate_name(
        _as_str(raw.get("slot"), "cache recovery journal.slot"), "slot"
    )
    if kind == "create":
        slot_type = _as_str(
            raw.get("slot_type", "agent"), "cache recovery journal.slot_type"
        )
        checkouts = tuple(
            _checkout_from_obj(item, f"cache recovery journal.created[{index}]")
            for index, item in enumerate(
                _as_list(raw.get("created"), "cache recovery journal.created")
            )
        )
    elif kind in {"import-existing", "finish"}:
        record = _record_from_obj(raw.get("record"), "cache recovery journal.record")
        slot_type = record.slot_type
        if record.slot != slot or record.machine != selected_machine:
            raise StateError(
                f"{kind} journal cache record does not match its shard or slot"
            )
        _assert_record_paths(config, record)
        removed = (
            {
                _as_str(item, f"cache recovery journal.removed[{index}]")
                for index, item in enumerate(
                    _as_list(raw.get("removed"), "cache recovery journal.removed")
                )
            }
            if kind == "finish"
            else set()
        )
        if not removed <= {checkout.name for checkout in record.checkouts}:
            raise StateError("finish journal cache record names an unknown removed checkout")
        checkouts = tuple(
            checkout for checkout in record.checkouts if checkout.name not in removed
        )
    else:
        raise StateError(f"unsupported cache recovery journal kind {kind!r}")
    journal_layout = (
        _record_layout(config, record)
        if kind in {"import-existing", "finish"}
        else config.layout
    )
    if journal_layout == "flat" and len(checkouts) > 1:
        raise StateError("flat-layout create journal records multiple checkouts")
    vcs = _GitVcs()
    present: list[Checkout] = []
    for checkout in checkouts:
        expected, destination = _checkout_path_for_layout(
            config, slot, checkout.name, slot_type, journal_layout
        )
        if checkout.path != expected:
            raise StateError(
                f"cache recovery journal checkout {checkout.name} escapes slot {slot}"
            )
        _relative, repository = _stored_repository_path(config, checkout.repository)
        if not destination.exists() and not destination.is_symlink():
            if destination.absolute() in vcs.listed_worktrees(repository):
                raise Refusal(
                    f"journaled checkout {checkout.name} is missing but Git still registers it"
                )
            continue
        if destination.is_symlink() or not destination.is_dir():
            raise Refusal(f"journaled checkout is unsafe for cache cleanup: {destination}")
        vcs.verify_existing_worktree(repository, destination)
        present.append(checkout)
    return CacheSlot(
        slot=slot,
        machine=selected_machine,
        checkouts=tuple(present),
        state=f"{kind}-journal",
    )


def _all_journal_cache_slots(config: Config) -> tuple[CacheSlot, ...]:
    slots: list[CacheSlot] = []
    seen: dict[str, str] = {}
    for path in _outstanding_journals(config):
        match = re.fullmatch(
            r"ACTIVE\.([A-Za-z0-9][A-Za-z0-9._-]{0,63})\.journal", path.name
        )
        if match is None:
            raise StateError(f"invalid recovery journal filename: {path}")
        cache_slot = _journal_cache_slot(config, match.group(1))
        assert cache_slot is not None
        previous = seen.get(cache_slot.slot)
        if previous is not None:
            raise StateError(
                f"slot {cache_slot.slot!r} has recovery journals on both "
                f"{previous} and {cache_slot.machine}"
            )
        seen[cache_slot.slot] = cache_slot.machine
        slots.append(cache_slot)
    return tuple(slots)


def _open_parent_directory(
    root: Path,
    path: Path,
    label: str,
    *,
    expected_root_identity: tuple[int, int, int] | None = None,
) -> tuple[int, str, int]:
    try:
        relative = path.relative_to(root)
    except ValueError as exc:
        raise Refusal(f"{label} escapes its managed root: {path}") from exc
    if not relative.parts:
        raise Refusal(f"{label} must not be the managed root")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW
    try:
        current_fd = os.open(root, flags)
    except OSError as exc:
        raise Refusal(f"cannot open managed root {root}: {exc}") from exc
    try:
        root_metadata = os.fstat(current_fd)
        root_mount_id = _fd_mount_id(current_fd, str(root))
        if expected_root_identity is not None:
            actual_root_identity = (
                root_metadata.st_dev,
                root_metadata.st_ino,
                root_mount_id,
            )
            if actual_root_identity != expected_root_identity:
                raise Refusal(f"{label} checkout identity changed before cleanup: {root}")
        for part in relative.parts[:-1]:
            next_fd = os.open(part, flags, dir_fd=current_fd)
            if _fd_mount_id(next_fd, str(path)) != root_mount_id:
                os.close(next_fd)
                raise Refusal(f"{label} crossed a mount point while opening {path}")
            os.close(current_fd)
            current_fd = next_fd
        return current_fd, relative.parts[-1], root_mount_id
    except OSError as exc:
        os.close(current_fd)
        raise Refusal(f"cannot safely open {label} parent for {path}: {exc}") from exc
    except Refusal:
        os.close(current_fd)
        raise


def _open_cache_directory(
    config: Config, cache: CacheDirectory
) -> tuple[int, int, str] | None:
    path = cache.path
    _ensure_no_symlink_components(cache.checkout_root, path, "cache path")
    _ensure_no_mount_components(cache.checkout_root, path, "cache path")
    _assert_no_mountinfo_crossing(cache.checkout_root, path, "cache path")
    parent_fd, name, checkout_mount_id = _open_parent_directory(
        cache.checkout_root,
        path,
        "cache path",
        expected_root_identity=(
            cache.checkout_device,
            cache.checkout_inode,
            cache.checkout_mount_id,
        ),
    )
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW
    try:
        cache_fd = os.open(name, flags, dir_fd=parent_fd)
    except FileNotFoundError:
        os.close(parent_fd)
        return None
    except OSError as exc:
        os.close(parent_fd)
        raise Refusal(f"cannot safely open cache directory {path}: {exc}") from exc
    if _fd_mount_id(cache_fd, str(path)) != checkout_mount_id:
        os.close(cache_fd)
        os.close(parent_fd)
        raise Refusal(f"cache path crossed a mount point while opening {path}")
    return parent_fd, cache_fd, name


def _directory_names(directory_fd: int, path: Path) -> list[str]:
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW
    try:
        scan_fd = os.open(".", flags, dir_fd=directory_fd)
    except OSError as exc:
        raise Refusal(f"cannot open cache directory {path} for enumeration: {exc}") from exc
    try:
        with os.scandir(scan_fd) as entries:
            return sorted(entry.name for entry in entries)
    except OSError as exc:
        raise Refusal(f"cannot enumerate cache directory {path}: {exc}") from exc
    finally:
        os.close(scan_fd)


def _allocated_open_directory(
    directory_fd: int, path: Path, device: int, mount_id: int
) -> int:
    try:
        metadata = os.fstat(directory_fd)
        if metadata.st_dev != device:
            raise Refusal(f"cache path crossed onto another filesystem: {path}")
        if _fd_mount_id(directory_fd, str(path)) != mount_id:
            raise Refusal(f"cache path crossed a mount point: {path}")
        total = metadata.st_blocks * 512
        names = _directory_names(directory_fd, path)
        for name in names:
            if name == ".git":
                raise Refusal(f"cache directory contains nested Git metadata: {path / name}")
            try:
                child = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
            except FileNotFoundError:
                continue
            except OSError as exc:
                raise Refusal(f"cannot inspect cache entry {path / name}: {exc}") from exc
            if not stat.S_ISDIR(child.st_mode):
                total += child.st_blocks * 512
                continue
            flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW
            try:
                child_fd = os.open(name, flags, dir_fd=directory_fd)
            except OSError as exc:
                raise Refusal(f"cache directory changed during inspection: {path / name}: {exc}") from exc
            try:
                opened = os.fstat(child_fd)
                if (opened.st_dev, opened.st_ino) != (child.st_dev, child.st_ino):
                    raise Refusal(f"cache directory changed during inspection: {path / name}")
                if _fd_mount_id(child_fd, str(path / name)) != mount_id:
                    raise Refusal(f"cache path crossed a mount point: {path / name}")
                total += _allocated_open_directory(
                    child_fd, path / name, device, mount_id
                )
            finally:
                os.close(child_fd)
        return total
    except OSError as exc:
        raise Refusal(f"cannot measure cache directory {path}: {exc}") from exc


def _allocated_cache_bytes(config: Config, cache: CacheDirectory) -> int:
    path = cache.path
    opened = _open_cache_directory(config, cache)
    if opened is None:
        return 0
    parent_fd, cache_fd, _name = opened
    try:
        device = os.fstat(cache_fd).st_dev
        mount_id = _fd_mount_id(cache_fd, str(path))
        return _allocated_open_directory(cache_fd, path, device, mount_id)
    finally:
        os.close(cache_fd)
        os.close(parent_fd)


def _clear_open_directory(
    directory_fd: int, path: Path, device: int, mount_id: int
) -> int:
    metadata = os.fstat(directory_fd)
    if metadata.st_dev != device:
        raise Refusal(f"cache path crossed onto another filesystem: {path}")
    if _fd_mount_id(directory_fd, str(path)) != mount_id:
        raise Refusal(f"cache path crossed a mount point: {path}")
    total = metadata.st_blocks * 512
    names = _directory_names(directory_fd, path)
    for name in names:
        if name == ".git":
            raise Refusal(f"cache directory contains nested Git metadata: {path / name}")
        try:
            child = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
        except FileNotFoundError:
            continue
        except OSError as exc:
            raise Refusal(f"cannot inspect cache entry {path / name}: {exc}") from exc
        if stat.S_ISDIR(child.st_mode):
            flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW
            try:
                child_fd = os.open(name, flags, dir_fd=directory_fd)
            except OSError as exc:
                raise Refusal(f"cache directory changed during cleanup: {path / name}: {exc}") from exc
            try:
                opened = os.fstat(child_fd)
                if (opened.st_dev, opened.st_ino) != (child.st_dev, child.st_ino):
                    raise Refusal(f"cache directory changed during cleanup: {path / name}")
                if _fd_mount_id(child_fd, str(path / name)) != mount_id:
                    raise Refusal(f"cache path crossed a mount point: {path / name}")
                total += _clear_open_directory(child_fd, path / name, device, mount_id)
            finally:
                os.close(child_fd)
            try:
                current = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
            except FileNotFoundError:
                continue
            if (
                not stat.S_ISDIR(current.st_mode)
                or (current.st_dev, current.st_ino) != (child.st_dev, child.st_ino)
            ):
                raise Refusal(f"cache directory changed during cleanup: {path / name}")
            try:
                os.rmdir(name, dir_fd=directory_fd)
            except OSError as exc:
                raise Refusal(f"cannot remove cache directory {path / name}: {exc}") from exc
        else:
            try:
                os.unlink(name, dir_fd=directory_fd)
            except FileNotFoundError:
                continue
            except OSError as exc:
                raise Refusal(f"cannot remove cache entry {path / name}: {exc}") from exc
            total += child.st_blocks * 512
    if _directory_names(directory_fd, path):
        raise Refusal(f"cache directory changed while it was being cleaned: {path}")
    return total


def _remove_cache_directory(config: Config, cache: CacheDirectory) -> int:
    path = cache.path
    opened = _open_cache_directory(config, cache)
    if opened is None:
        return 0
    parent_fd, cache_fd, name = opened
    try:
        original = os.fstat(cache_fd)
        mount_id = _fd_mount_id(cache_fd, str(path))
        # Complete a read-only traversal before deletion begins. The destructive
        # traversal repeats every identity check so replacements still fail closed.
        _allocated_open_directory(cache_fd, path, original.st_dev, mount_id)
        size = _clear_open_directory(cache_fd, path, original.st_dev, mount_id)
        try:
            current = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
        except FileNotFoundError:
            return size
        if (
            not stat.S_ISDIR(current.st_mode)
            or (current.st_dev, current.st_ino) != (original.st_dev, original.st_ino)
        ):
            raise Refusal(f"cache directory changed before final removal: {path}")
        try:
            os.rmdir(name, dir_fd=parent_fd)
            os.fsync(parent_fd)
        except OSError as exc:
            raise Refusal(f"cannot remove configured cache directory {path}: {exc}") from exc
        return size
    finally:
        os.close(cache_fd)
        os.close(parent_fd)


def _cmd_clean_caches(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    if not config.cache_globs and not config.repo_cache_globs:
        raise Refusal("no cache globs are configured")
    only = tuple(args.only or ())
    for slot in only:
        _validate_name(slot, "slot")
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        states, _archives = _validate_global_state(config)
        cache_slots = {
            record.slot: CacheSlot(record.slot, record.machine, record.checkouts)
            for state in states
            for record in state.slots
        }
        registered_slots = {
            record.slot for candidate in states for record in candidate.slots
        }
        journal_slots = _all_journal_cache_slots(config)
        registered_slots.update(slot.slot for slot in journal_slots)
        for journal_slot in journal_slots:
            cache_slots[journal_slot.slot] = journal_slot
        malformed_slots: dict[str, str] = {}
        for entry in sorted(config.worktrees.iterdir(), key=lambda path: path.name):
            if (
                not entry.is_dir()
                or entry.is_symlink()
                or entry.name.startswith("EVENTS.")
                or entry == _validate_slots_directory(config)
                or entry.name in registered_slots
            ):
                continue
            try:
                _validate_name(entry.name, "unregistered slot")
            except Refusal as exc:
                if args.yes:
                    raise Refusal(
                        f"cannot bulk-clean malformed unregistered directory {entry}: {exc}"
                    ) from exc
                malformed_slots[entry.name] = str(exc)
                continue
            cache_slots[entry.name] = CacheSlot(
                entry.name,
                config.machine,
                (),
                "unregistered",
            )
        unknown = sorted(set(only) - set(cache_slots))
        if unknown:
            raise Refusal(f"unknown slot(s) for cache cleanup: {', '.join(unknown)}")
        selected = set(cache_slots) if args.yes else set(only)
        rows: list[dict[str, object]] = [
                {
                    "slot": slot,
                    "machine": None,
                    "cache_bytes": 0,
                "action": "BLOCKED",
                "paths": [],
                "hold_reason": None,
                "registered": False,
                "state": "unregistered",
                "cache_error": error,
                "policy_note": None,
            }
            for slot, error in sorted(malformed_slots.items())
        ]
        plans: dict[str, tuple[CacheDirectory, ...]] = {}
        for slot in sorted(cache_slots):
            cache_slot = cache_slots[slot]
            hold = _load_hold(config, slot, cache_slot.machine)
            directories: tuple[CacheDirectory, ...]
            cache_error: str | None = None
            policy_note = (
                "repository-specific cache globs were not applied because this slot has no "
                "trusted repository-name mapping"
                if cache_slot.state == "unregistered" and config.repo_cache_globs
                else None
            )
            if hold is not None:
                directories = ()
                cache_bytes = 0
                action = "HELD"
            else:
                try:
                    directories = _cache_slot_directories(config, cache_slot)
                    cache_bytes = sum(
                        _allocated_cache_bytes(config, cache) for cache in directories
                    )
                except Refusal as exc:
                    if slot in selected:
                        raise Refusal(
                            f"cannot clean selected slot {slot}: {exc}"
                        ) from exc
                    directories = ()
                    cache_bytes = 0
                    cache_error = str(exc)
                    action = "BLOCKED"
                else:
                    if slot in selected:
                        action = "REMOVED"
                        plans[slot] = directories
                    else:
                        action = "REPORT"
            rows.append(
                {
                    "slot": slot,
                    "machine": cache_slot.machine,
                    "cache_bytes": cache_bytes,
                    "action": action,
                    "paths": [str(cache.path) for cache in directories],
                    "hold_reason": None if hold is None else hold["reason"],
                    "registered": cache_slot.state == "active",
                    "state": cache_slot.state,
                    "cache_error": cache_error,
                    "policy_note": policy_note,
                }
            )
        removed_bytes = 0
        for row in rows:
            slot = _as_str(row["slot"], "cache row slot")
            for cache in plans.get(slot, ()):
                removed_bytes += _remove_cache_directory(config, cache)
    payload = {
        "schema": SCHEMA,
        "cache_globs": list(config.cache_globs),
        "repo_cache_globs": {
            name: list(patterns) for name, patterns in config.repo_cache_globs
        },
        "removed_bytes": removed_bytes,
        "slots": rows,
    }
    if args.format == "json":
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        mode = "delete" if selected else "report-only"
        print(
            f"cache_mode={mode} slots={len(rows)} removed={_format_gib(removed_bytes)}"
        )
        for row in rows:
            print(
                f"{row['action']}: {row['slot']} cache={_format_gib(_as_int(row['cache_bytes'], 'cache bytes'))}"
                + (
                    f" reason={row['hold_reason']}"
                    if row["hold_reason"] is not None
                    else ""
                )
                + (
                    f" error={row['cache_error']}"
                    if row["cache_error"] is not None
                    else ""
                )
                + (
                    f" note={row['policy_note']}"
                    if row["policy_note"] is not None
                    else ""
                )
            )
    return 0


def _audit_record(config: Config, record: ActiveRecord) -> tuple[dict[str, object], bool]:
    owner_state, owner_detail = _process_state(record.owner)
    liveness_state, liveness_detail = _registered_liveness_state(config, record)
    heartbeat_age, heartbeat_expired = _heartbeat_diagnosis(record)
    agent_running = not (
        liveness_state == "dead"
        and owner_state == "dead"
    )
    hold = _load_hold(config, record.slot, record.machine)
    cache_bytes = 0
    cache_error: str | None = None
    try:
        cache_bytes = sum(
            _allocated_cache_bytes(config, cache)
            for cache in _cache_directories(config, record.checkouts)
        )
    except Refusal as exc:
        cache_error = str(exc)
    if hold is not None:
        return (
            {
                "slot": record.slot,
                "machine": record.machine,
                "agent": record.agent,
                "slot_type": record.slot_type,
                "verdict": "HELD",
                "reasons": [_as_str(hold["reason"], "slot hold.reason")],
                "owner_state": owner_state,
                "liveness_state": liveness_state,
                "heartbeat_age_seconds": int(heartbeat_age),
                "heartbeat_expired": heartbeat_expired,
                "cache_bytes": cache_bytes,
                "cache_error": cache_error,
            },
            agent_running,
        )
    reasons: list[str] = []
    if not heartbeat_expired:
        reasons.append(
            f"heartbeat age {int(heartbeat_age)}s has not exceeded the "
            f"{record.heartbeat_ttl_seconds}s time-to-live"
        )
    if liveness_state != "dead":
        reasons.append(f"liveness is {liveness_state}: {liveness_detail}")
    if record.owner is None:
        reasons.append("no owner process generation is recorded, so death is unknown")
    elif owner_state != "dead":
        reasons.append(f"recorded owner is {owner_state}: {owner_detail}")
    slot_path = _slot_directory(config, record.slot, record.slot_type)
    try:
        _assert_record_paths(config, record)
        _assert_slot_contents(config, record)
        _assert_handoff_read(config, record, slot_path)
        vcs = _GitVcs()
        for checkout in record.checkouts:
            path = _stored_path(config, checkout.path, "checkout path")
            repository = _stored_repository_path(config, checkout.repository)[1]
            vcs.verify_existing_worktree(repository, path)
            if vcs.branch(path) != checkout.branch:
                raise Refusal(f"checkout {checkout.name} branch changed")
            if record.slot_type == "agent":
                operations = vcs.operation_paths(path)
                if operations:
                    raise Refusal(
                        f"checkout {checkout.name} has an unfinished Git operation: "
                        f"{operations[0]}"
                    )
                vcs.assert_ordinary_history(path)
                vcs.assert_ordinary_index(path)
                _assert_cache_policy_untracked(config, checkout, vcs)
            if vcs.remote_url_sha256(path, checkout.remote) != checkout.remote_url_sha256:
                raise Refusal(f"checkout {checkout.name} remote URL changed")
    except Refusal as exc:
        reasons.append(str(exc))
    if (
        heartbeat_expired
        and liveness_state == "dead"
        and owner_state == "dead"
        and not reasons
    ):
        try:
            _assert_slot_unused(slot_path, record)
        except Refusal as exc:
            reasons.append(str(exc))
            agent_running = True
    if cache_error is not None:
        reasons.append(f"cache inspection failed: {cache_error}")
    return (
        {
            "slot": record.slot,
            "machine": record.machine,
            "agent": record.agent,
            "slot_type": record.slot_type,
            "verdict": "DELETABLE" if not reasons else "BLOCKED",
            "reasons": reasons,
            "owner_state": owner_state,
            "liveness_state": liveness_state,
            "heartbeat_age_seconds": int(heartbeat_age),
            "heartbeat_expired": heartbeat_expired,
            "cache_bytes": cache_bytes,
            "cache_error": cache_error,
        },
        agent_running,
    )


def _cmd_audit(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    with _locked(config.control / "ACTIVE", exclusive=False, wait_seconds=args.wait_lock):
        _refuse_partial_state(config)
        states, _archives = _validate_global_state(config)
        records = [record for state in states for record in state.slots]
        journal_slots = {slot.slot: slot for slot in _all_journal_cache_slots(config)}
        rows: list[dict[str, object]] = []
        running_agent_count = 0
        for record in records:
            row, running = _audit_record(config, record)
            journal_slot = journal_slots.get(record.slot)
            if journal_slot is not None:
                reasons = row["reasons"]
                assert isinstance(reasons, list)
                reasons.insert(
                    0,
                    f"interrupted {journal_slot.state.replace('-', ' ')} requires "
                    "wrkslots recover",
                )
                row["verdict"] = "BLOCKED"
            rows.append(row)
            running_agent_count += int(running)
        on_disk: set[tuple[str, str]] = set()
        for slot_type, root in _slot_roots(config).items():
            if not root.exists():
                continue
            try:
                on_disk.update(
                    (slot_type, entry.name)
                    for entry in root.iterdir()
                    if entry.is_dir()
                    and not entry.is_symlink()
                    and not entry.name.startswith("EVENTS.")
                    and not (slot_type == "agent" and entry == _validate_slots_directory(config))
                )
            except OSError as exc:
                raise Refusal(f"cannot enumerate worktree slots in {root}: {exc}") from exc
        registered = {(record.slot_type, record.slot) for record in records}
        for slot_type, slot in sorted(on_disk - registered):
            journal_slot = journal_slots.get(slot)
            cache_bytes = 0
            cache_error: str | None = None
            if slot_type == "agent":
                try:
                    unregistered = (
                        journal_slot
                        if journal_slot is not None
                        else CacheSlot(slot, config.machine, (), "unregistered")
                    )
                    cache_bytes = sum(
                        _allocated_cache_bytes(config, cache)
                        for cache in _cache_slot_directories(config, unregistered)
                    )
                except Refusal as exc:
                    cache_error = str(exc)
            rows.append(
                {
                    "slot": slot,
                    "slot_type": slot_type,
                    "machine": None if journal_slot is None else journal_slot.machine,
                    "agent": None,
                    "verdict": "BLOCKED",
                    "reasons": [
                        (
                            f"interrupted {journal_slot.state.replace('-', ' ')} requires "
                            "wrkslots recover"
                            if journal_slot is not None
                            else "directory has no active registry row; inspect or import it"
                        )
                    ],
                    "owner_state": (
                        journal_slot.state
                        if journal_slot is not None
                        else "unregistered"
                    ),
                    "liveness_state": "unverifiable",
                    "cache_bytes": cache_bytes,
                    "cache_error": cache_error,
                }
            )
        for journal_slot in sorted(journal_slots.values(), key=lambda item: item.slot):
            if any(
                slot == journal_slot.slot
                for _slot_type, slot in registered | on_disk
            ):
                continue
            rows.append(
                {
                    "slot": journal_slot.slot,
                    "slot_type": None,
                    "machine": journal_slot.machine,
                    "agent": None,
                    "verdict": "BLOCKED",
                    "reasons": [
                        f"interrupted {journal_slot.state.replace('-', ' ')} requires "
                        "wrkslots recover"
                    ],
                    "owner_state": journal_slot.state,
                    "liveness_state": "unverifiable",
                    "cache_bytes": 0,
                    "cache_error": None,
                }
            )
        worktree_count = len(on_disk)
        disk = _disk_status(config)
    sorted_rows = sorted(rows, key=lambda row: str(row["slot"]))
    attention = [
        row
        for row in sorted_rows
        if row["verdict"] == "DELETABLE"
        or row["owner_state"] == "unregistered"
        or any(
            str(reason).startswith("interrupted ")
            for reason in _as_list(row["reasons"], "audit row.reasons")
        )
    ]
    unknown = [
        row
        for row in sorted_rows
        if row["verdict"] == "BLOCKED"
        and row.get("heartbeat_expired") is True
        and (
            row["owner_state"] not in ("live", "dead")
            or row["liveness_state"] not in ("alive", "dead")
            or row["cache_error"] is not None
            or any(
                token in str(reason).lower()
                for reason in _as_list(row["reasons"], "audit row.reasons")
                for token in ("cannot", "failed", "unavailable", "unknown")
            )
        )
    ]
    attention_ids = {(str(row["slot"]), str(row["machine"])) for row in attention}
    unknown = [
        row
        for row in unknown
        if (str(row["slot"]), str(row["machine"])) not in attention_ids
    ]
    attention_names = [str(row["slot"]) for row in attention]
    unknown_names = [str(row["slot"]) for row in unknown]
    if attention_names:
        gate_state = "actionable"
        summary = (
            f"{len(attention_names)} worktree slot(s) need coordinator attention: "
            + ", ".join(attention_names)
        )
    elif unknown_names:
        gate_state = "unknown"
        summary = (
            f"could not determine reclaim state for {len(unknown_names)} worktree "
            "slot(s): " + ", ".join(unknown_names)
        )
    else:
        gate_state = "ok"
        summary = "no worktree slot currently needs coordinator attention"
    payload = {
        "schema": SCHEMA,
        "worktree_count": worktree_count,
        "running_agent_count": running_agent_count,
        "leak": worktree_count > running_agent_count,
        "attention_count": len(attention_names),
        "attention_slots": attention_names,
        "unknown_count": len(unknown_names),
        "unknown_slots": unknown_names,
        "state": gate_state,
        "summary": summary,
        "remote_refs_refreshed": False,
        "disk": disk,
        "slots": sorted_rows,
    }
    if args.format == "json":
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(
            f"worktrees={worktree_count} running_agents={running_agent_count} "
            f"leak={'yes' if payload['leak'] else 'no'} disk={disk['state']} "
            f"free={_format_gib(_as_int(disk['free_bytes'], 'disk free bytes'))}"
        )
        for row in sorted_rows:
            reasons = row["reasons"]
            assert isinstance(reasons, list)
            detail = "; ".join(str(reason) for reason in reasons) or "all removal proofs pass"
            if row["cache_error"] is not None:
                detail += f"; cache inspection: {row['cache_error']}"
            print(f"{row['verdict']}: {row['slot']}: {detail}")
        if args.gate:
            print(f"state={gate_state}")
            print(f"summary={summary}")
            if attention_names:
                print(
                    "ACTION: run 'wrkslots audit --format json' for exact evidence; "
                    "use 'wrkslots recover' for interrupted operations, import live "
                    "unregistered slots, and run 'wrkslots remove' only for a slot "
                    "reported DELETABLE"
                )
            elif unknown_names:
                print(
                    "ACTION: restore the unavailable liveness or process evidence and "
                    "rerun 'wrkslots audit --gate'; do not remove an UNKNOWN slot"
                )
    if not args.gate:
        return 0
    if attention_names:
        return 1
    return 2 if unknown_names else 0


def _cmd_unpushed(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    if args.slot is not None:
        _validate_name(args.slot, "slot")
    with _locked(config.control / "ACTIVE", exclusive=False, wait_seconds=args.wait_lock):
        _refuse_partial_state(config)
        states, _archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        records = [
            record
            for state in states
            if state.machine == config.machine
            for record in state.slots
            if args.slot is None or record.slot == args.slot
        ]
        rows: list[dict[str, object]] = []
        vcs = _GitVcs()
        for record in records:
            for checkout in record.checkouts:
                path = _stored_path(config, checkout.path, "checkout path")
                head = vcs.verify_existing_worktree(
                    _stored_repository_path(config, checkout.repository)[1], path
                )
                branch = vcs.branch(path)
                if branch != checkout.branch:
                    raise Refusal(
                        f"checkout {checkout.name} branch changed: expected "
                        f"{checkout.branch}, found {branch}; preserve it for inspection"
                    )
                if vcs.remote_url_sha256(path, checkout.remote) != checkout.remote_url_sha256:
                    raise Refusal(
                        f"checkout {checkout.name} remote {checkout.remote} URL changed"
                    )
                vcs.fetch_remote(path, checkout.remote, checkout.landed_ref)
                if vcs.remote_url_sha256(path, checkout.remote) != checkout.remote_url_sha256:
                    raise Refusal(
                        f"checkout {checkout.name} remote {checkout.remote} URL changed during fetch"
                    )
                containing = vcs.remote_refs_containing(path, checkout.remote, head)
                evidence = _unpushed_evidence(checkout, path, head, vcs)
                rows.append(
                    {
                        "slot": record.slot,
                        "checkout": checkout.name,
                        "head": head,
                        "state": "PUBLISHED" if containing else "UNPUSHED-OR-STALE",
                        "containing_remote_refs": list(containing),
                        "same_named_remote_ref": evidence["same_named_remote_ref"],
                        "touched_files": evidence["touched_files"],
                        "diagnostic_command": (
                            None if containing else evidence["diagnostic_command"]
                        ),
                    }
                )
    payload = {"schema": SCHEMA, "slots": rows}
    if args.format == "json":
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        for row in rows:
            print(f"{row['state']}: {row['slot']}/{row['checkout']} HEAD={row['head']}")
            if row["state"] == "UNPUSHED-OR-STALE":
                if row["same_named_remote_ref"] is None:
                    print("  no same-named remote ref exists; publish the branch")
                else:
                    print(
                        f"  same-named remote ref={row['same_named_remote_ref']}; two readings: "
                        "genuinely unpushed or stale after remote rebase"
                    )
                    print(
                        f"  compare={row['diagnostic_command']} (empty output means content present)"
                    )
    return 0


def _status_record(
    config: Config, record: ActiveRecord, *, tolerate_hold_error: bool = False
) -> dict[str, object]:
    process_state, process_detail = _process_state(record.owner)
    age, expired = _heartbeat_diagnosis(record)
    value = _record_to_obj(record)
    value["owner_state"] = process_state
    value["owner_detail"] = process_detail
    value["heartbeat_age_seconds"] = int(age)
    value["heartbeat_expired"] = expired
    try:
        hold = _load_hold(config, record.slot, record.machine)
        hold_error: str | None = None
    except Refusal as exc:
        if not tolerate_hold_error:
            raise
        hold = None
        hold_error = str(exc)
    value["held"] = hold is not None
    value["hold_reason"] = None if hold is None else hold["reason"]
    value["hold_error"] = hold_error
    return value


def _slot_findings(config: Config, states: Sequence[ActiveState]) -> list[dict[str, object]]:
    """Every registry/storage disagreement, as data rather than as a refusal.

    ``status`` deliberately refuses on the first disagreement, because a caller
    that is about to act must not act on a model known to be wrong. That
    refusal is the right contract for status and is left exactly as it was.
    But it names one problem and hides the rest, so an operator repairing a
    registry that has drifted learns about the drift one run at a time, and
    cannot see the shape of it at all.

    This collects the same conditions without raising and without authorizing
    anything. It is diagnosis only: nothing here is a precondition for removal,
    and no caller may treat a clean result as permission.
    """
    findings: list[dict[str, object]] = []

    def note(kind: str, slot: str | None, machine: str | None, detail: str) -> None:
        findings.append(
            {"kind": kind, "slot": slot, "machine": machine, "detail": detail}
        )

    expected: dict[str, set[str]] = {slot_type: set() for slot_type in SLOT_TYPES}
    for state in states:
        for record in state.slots:
            expected[record.slot_type].add(record.slot)
            try:
                _load_hold(config, record.slot, record.machine)
            except Refusal as exc:
                note("invalid-hold", record.slot, state.machine, str(exc))
            slot_path = _slot_directory(config, record.slot, record.slot_type)
            if not slot_path.is_dir() or slot_path.is_symlink():
                note(
                    "row-without-directory",
                    record.slot,
                    state.machine,
                    f"active row but no slot directory at {slot_path}",
                )
                continue
            record_layout = _record_layout(config, record)
            if record_layout == "flat":
                if len(record.checkouts) != 1:
                    note(
                        "flat-layout-cardinality",
                        record.slot,
                        state.machine,
                        "flat layout requires exactly one checkout",
                    )
            else:
                try:
                    actual = {entry.name for entry in slot_path.iterdir()}
                except OSError as exc:
                    note(
                        "slot-unreadable", record.slot, state.machine,
                        f"cannot inspect {slot_path}: {exc}",
                    )
                    continue
                want = {checkout.name for checkout in record.checkouts}
                if record.slot_type == "agent":
                    want.add("HANDOFF.md")
                for name in sorted(want - actual):
                    if name == "HANDOFF.md":
                        continue
                    note(
                        "missing-checkout", record.slot, state.machine,
                        f"record names checkout {name!r} but it is not in the slot",
                    )
                for name in sorted(actual - want):
                    note(
                        "unexpected-entry", record.slot, state.machine,
                        f"slot holds {name!r}, which its record does not name",
                    )
            process_state, process_detail = _process_state(record.owner)
            if process_state == "dead":
                note(
                    "dead-owner", record.slot, state.machine,
                    f"recorded owner is not running ({process_detail}); the slot "
                    "is held by a row whose owner has exited",
                )

    for slot_type, root in _slot_roots(config).items():
        try:
            on_disk = {
                entry.name
                for entry in root.iterdir()
                if entry.is_dir()
                and not entry.is_symlink()
                and not entry.name.startswith("EVENTS.")
                and not (slot_type == "agent" and entry == _validate_slots_directory(config))
            }
        except FileNotFoundError:
            on_disk = set()
        except OSError as exc:
            note("worktrees-unreadable", None, None, f"cannot enumerate {root}: {exc}")
            on_disk = set()
        for name in sorted(on_disk - expected[slot_type]):
            note(
                "directory-without-row",
                name,
                None,
                f"{root / name} exists but no active {slot_type} row claims it",
            )

    for path in _outstanding_journals(config):
        note("unfinished-journal", None, None, f"recovery required: {path.name}")

    # `status` refuses outright on a partial atomic update. Diagnosis reports
    # it instead, because an operator looking at a wedged registry needs to see
    # the interrupted write alongside everything else, not in place of it.
    leftovers = sorted(config.control.glob("ACTIVE.*.json.tmp.*"))
    leftovers += sorted(config.control.glob("ARCHIVED.*.json.tmp.*"))
    leftovers += sorted(config.control.glob("ACTIVE.*.journal.tmp.*"))
    for event_directory in sorted(config.control.glob("EVENTS.*")):
        if event_directory.is_dir() and not event_directory.is_symlink():
            leftovers += sorted(event_directory.glob("*.json.tmp.*"))
    for path in leftovers:
        note(
            "partial-atomic-update", None, None,
            f"interrupted write left {path.name}; preserve it and run "
            "'wrkslots recover'",
        )

    cap = config.max_active_slots
    mine = [r for s in states if s.machine == config.machine for r in s.slots]
    if cap is not None and len(mine) > cap:
        note(
            "cap-breach", None, config.machine,
            f"{len(mine)} active slot(s) against max_active_slots={cap} "
            f"(excess {len(mine) - cap})",
        )
    return findings


def _cmd_doctor(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    with _locked(
        config.control / "ACTIVE", exclusive=False, wait_seconds=args.wait_lock
    ):
        all_states, _archives = _validate_global_state(config)
        states = (
            all_states
            if args.all_machines
            else [state for state in all_states if state.machine == config.machine]
        )
        findings = _slot_findings(config, states)
        rows = [
            _status_record(config, record, tolerate_hold_error=True)
            for state in states
            for record in state.slots
        ]
    cap = config.max_active_slots
    if args.format == "json":
        print(
            json.dumps(
                {
                    "schema": SCHEMA,
                    "project_root": str(config.root),
                    "worktrees_dir": str(config.worktrees),
                    "control_dir": str(config.control),
                    "layout": config.layout,
                    "machines": [state.machine for state in states],
                    "event_counts": {
                        state.machine: len(_load_events(config, state.machine))
                        for state in states
                    },
                    "active": rows,
                    "max_active_slots": cap,
                    "findings": findings,
                },
                indent=2,
                sort_keys=True,
            )
        )
    else:
        print(
            f"project={config.root} worktrees={config.worktrees} control={config.control} "
            f"layout={config.layout} "
            f"active={len(rows)} cap={'none' if cap is None else cap} "
            f"findings={len(findings)}"
        )
        counts: dict[str, int] = {}
        for item in findings:
            kind = str(item["kind"])
            counts[kind] = counts.get(kind, 0) + 1
        for kind in sorted(counts):
            print(f"  {counts[kind]:4d}  {kind}")
        for item in findings:
            where = item["slot"] or "-"
            print(f"{item['kind']}: {where}: {item['detail']}")
    # Diagnosis never authorizes anything, so it does not fail the caller for
    # finding problems; a non-zero exit here would push callers toward `|| true`
    # and lose the real refusals too.
    return 0


def _cmd_status(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    with _locked(
        config.control / "ACTIVE", exclusive=False, wait_seconds=args.wait_lock
    ):
        _refuse_partial_state(config)
        all_states, _archives = _validate_global_state(config)
        _assert_command_registry_storage(config, all_states, args)
        states = (
            all_states
            if args.all_machines
            else [state for state in all_states if state.machine == config.machine]
        )
        journals = [path.name for path in _outstanding_journals(config)]
        records = [
            _status_record(config, record)
            for state in states
            for record in state.slots
            if args.slot is None or record.slot == args.slot
        ]
    if args.format == "json":
        print(
            json.dumps(
                {
                    "schema": SCHEMA,
                    "project_root": str(config.root),
                    "worktrees_dir": str(config.worktrees),
                    "control_dir": str(config.control),
                    "layout": config.layout,
                    "machines": [state.machine for state in states],
                    "active": records,
                    "journals": journals,
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    print(
        f"project={config.root} worktrees={config.worktrees} control={config.control} "
        f"layout={config.layout} "
        f"active={len(records)} journals={len(journals)} "
        f"events={sum(len(_load_events(config, state.machine)) for state in states)}"
    )
    if journals:
        for journal in journals:
            print(f"RECOVERY REQUIRED: {journal}")
    for value in records:
        owner_state = _as_str(value["owner_state"], "status owner_state")
        expired = value["heartbeat_expired"] is True
        print(
            f"{value['machine']}/{value['slot']}: type={value['slot_type']} "
            f"agent={value['agent']} task={value['task']} "
            f"generation={value['generation']} owner={owner_state} "
            f"heartbeat_age={value['heartbeat_age_seconds']}s expired={'yes' if expired else 'no'} "
            f"held={'yes' if value['held'] is True else 'no'}"
        )
    return 0


def _finish_journal_payload(
    config: Config,
    record: ActiveRecord,
    *,
    mode: str,
    actor: str,
    finished_at: str,
    removed: Sequence[str],
    phase: str,
    salvage: Sequence[Mapping[str, object]] = (),
    validate_complete: bool = False,
) -> dict[str, object]:
    archive_id = f"{config.machine}:{record.slot}:{record.generation}:{finished_at}"
    slot_parent = _slot_directory(config, record.slot, record.slot_type).parent
    fenced = (
        slot_parent
        / f".{record.slot}.fenced.{record.generation}.{uuid.uuid4().hex}"
    )
    return {
        "schema": SCHEMA,
        "kind": "finish",
        "machine": config.machine,
        "slot": record.slot,
        "mode": mode,
        "actor": actor,
        "finished_at": finished_at,
        "archive_id": archive_id,
        "phase": phase,
        "fenced": fenced.relative_to(config.root).as_posix(),
        "removed": list(removed),
        "salvage": [dict(item) for item in salvage],
        "validate_complete": validate_complete,
        "record": _record_to_obj(record),
    }


def _archive_entry(
    journal: Mapping[str, object], record: ActiveRecord
) -> dict[str, object]:
    if record.handoff is None:
        raise StateError("cannot archive a slot without durable reclaim evidence")
    value: dict[str, object] = {
        "archive_id": _as_str(journal["archive_id"], "journal.archive_id"),
        "slot": record.slot,
        "agent": record.agent,
        "task": record.task,
        "purpose": record.purpose,
        "slot_type": record.slot_type,
        "machine": record.machine,
        "generation": record.generation,
        "created_at": record.created_at,
        "finished_at": _as_str(journal["finished_at"], "journal.finished_at"),
        "mode": _as_str(journal["mode"], "journal.mode"),
        "actor": _as_str(journal["actor"], "journal.actor"),
        "physical_storage": "removed",
        "validation": list(record.handoff.validation),
        "limitations": list(record.handoff.limitations),
        "continuation": record.handoff.continuation,
        "salvage": [
            dict(_as_mapping(item, "finish journal.salvage entry"))
            for item in _as_list(journal.get("salvage", []), "finish journal.salvage")
        ],
        "checkouts": [_checkout_to_obj(item) for item in record.checkouts],
    }
    if record.layout is not None:
        value["layout"] = record.layout
    if record.import_source is not None:
        value["import_source"] = _import_source_to_obj(record.import_source)
    return value


def _append_archive_once(
    config: Config, archive: ArchiveState, entry: dict[str, object]
) -> ArchiveState:
    archive_id = _as_str(entry["archive_id"], "archive entry archive_id")
    for existing in archive.records:
        if existing.get("archive_id") == archive_id:
            if not _json_equal(existing, entry):
                raise StateError(
                    f"archive_id {archive_id!r} already exists with different content"
                )
            _atomic_write_json(_archive_path(config), _archive_to_obj(archive))
            return archive
    updated = ArchiveState(
        machine=archive.machine,
        revision=archive.revision + 1,
        records=(*archive.records, entry),
    )
    _write_archive_state(
        config,
        updated,
        action="slot-archived",
        slot=_as_str(entry["slot"], "archive entry slot"),
        evidence={"archive_id": archive_id},
    )
    return updated


def _finish_fenced_slot(
    config: Config, record: ActiveRecord, journal: Mapping[str, object]
) -> Path:
    relative, fenced = _relative_inside(
        config.root,
        _as_str(journal["fenced"], "finish journal.fenced"),
        "finish journal fenced path",
    )
    del relative
    prefix = f".{record.slot}.fenced.{record.generation}."
    slot_parent = _slot_directory(config, record.slot, record.slot_type).parent
    if (
        fenced.parent != slot_parent
        or not fenced.name.startswith(prefix)
        or len(fenced.name) != len(prefix) + 32
        or not re.fullmatch(r"[0-9a-f]{32}", fenced.name.removeprefix(prefix))
    ):
        raise StateError("finish journal fenced path does not match its slot generation")
    return fenced


def _checkout_at_slot(
    config: Config, record: ActiveRecord, checkout: Checkout, slot_path: Path
) -> tuple[Checkout, Path]:
    path = (
        slot_path
        if _record_layout(config, record) == "flat"
        else slot_path / checkout.name
    )
    relative = path.relative_to(config.root).as_posix()
    return dataclasses.replace(checkout, path=relative), path


def _repair_checkout_at_slot(
    config: Config,
    record: ActiveRecord,
    checkout: Checkout,
    slot_path: Path,
    vcs: _GitVcs,
) -> Path:
    moved, path = _checkout_at_slot(config, record, checkout, slot_path)
    _repair_registration_at_slot(config, record, checkout, slot_path, vcs)
    current = _assert_checkout_safe(config, moved, vcs)
    if dataclasses.replace(current, path=checkout.path) != checkout:
        raise Refusal(
            f"fenced checkout {checkout.name} changed after finish was prepared; preserve it for inspection"
        )
    return path


def _declared_initialized_submodules(checkout: Path) -> tuple[Path, ...]:
    found: list[Path] = []

    def visit(repository: Path) -> None:
        modules = repository / ".gitmodules"
        if not modules.exists() and not modules.is_symlink():
            return
        if modules.is_symlink() or not modules.is_file():
            raise Refusal(f"nested repository has an unsafe .gitmodules file: {modules}")
        parser = configparser.RawConfigParser(interpolation=None, strict=True)
        try:
            with modules.open("r", encoding="utf-8") as handle:
                parser.read_file(handle)
        except (OSError, UnicodeError, configparser.Error) as exc:
            raise Refusal(f"cannot read nested repository declarations {modules}: {exc}") from exc
        for section in parser.sections():
            if not section.startswith('submodule "') or not parser.has_option(section, "path"):
                continue
            raw = parser.get(section, "path")
            relative = Path(raw)
            if relative.is_absolute() or not relative.parts or ".." in relative.parts:
                raise Refusal(f"unsafe submodule path {raw!r} in {modules}")
            nested = repository / relative
            _ensure_no_symlink_components(repository, nested, "nested repository")
            if not nested.exists() and not nested.is_symlink():
                continue
            if nested.is_symlink() or not nested.is_dir():
                raise Refusal(f"nested repository path is unsafe: {nested}")
            git_file = nested / ".git"
            if not git_file.exists() and not git_file.is_symlink():
                continue
            found.append(nested)
            visit(nested)

    visit(checkout)
    return tuple(found)


def _repair_nested_repositories(checkout: Path, vcs: _GitVcs) -> None:
    for nested in _declared_initialized_submodules(checkout):
        vcs.repair_nested_repository(nested)


def _repair_registration_at_slot(
    config: Config,
    record: ActiveRecord,
    checkout: Checkout,
    slot_path: Path,
    vcs: _GitVcs,
) -> Path:
    _moved, path = _checkout_at_slot(config, record, checkout, slot_path)
    _relative, repository = _stored_repository_path(config, checkout.repository)
    vcs.repair_worktree(repository, path)
    _repair_nested_repositories(path, vcs)
    return path


def _remove_fenced_directory(config: Config, fenced_slot: Path) -> None:
    if fenced_slot.exists():
        try:
            fenced_slot.rmdir()
        except OSError as exc:
            raise Refusal(
                f"fenced slot directory is not empty after Git removal: {fenced_slot}: {exc}"
            ) from exc
    _fsync_directory(fenced_slot.parent)


def _rollback_path_fence(
    config: Config,
    record: ActiveRecord,
    journal: dict[str, object],
    vcs: _GitVcs,
) -> None:
    removed = {
        _as_str(item, "journal.removed item")
        for item in _as_list(journal["removed"], "journal.removed")
    }
    if removed:
        raise Refusal("cannot roll back a path fence after a checkout was removed")
    original = _slot_directory(config, record.slot, record.slot_type)
    fenced = _finish_fenced_slot(config, record, journal)
    if original.exists() or original.is_symlink():
        raise Refusal(f"canonical slot reappeared before path-fence rollback: {original}")
    if not fenced.is_dir() or fenced.is_symlink():
        raise Refusal(f"fenced slot is missing or unsafe during rollback: {fenced}")
    try:
        os.rename(fenced, original)
        _fsync_directory(original.parent)
    except OSError as exc:
        raise Refusal(f"cannot restore path fence {fenced} to {original}: {exc}") from exc
    journal["phase"] = "prepared"
    _write_journal(config, journal)
    for checkout in record.checkouts:
        # Rollback preserves the checkout; it does not authorize deletion. Repair
        # Git's path registration without requiring the content to still match the
        # pre-fence snapshot, because a changed checkout is exactly what must be
        # returned to its canonical path for inspection.
        _repair_registration_at_slot(config, record, checkout, original, vcs)
    _remove_fenced_directory(config, fenced)
    _clear_journal(config, journal)


def _begin_or_resume_path_fence(
    config: Config,
    record: ActiveRecord,
    journal: dict[str, object],
    vcs: _GitVcs,
    *,
    allow_live_validate_owner: bool = False,
) -> tuple[dict[str, object], Path]:
    original = _slot_directory(config, record.slot, record.slot_type)
    fenced = _finish_fenced_slot(config, record, journal)
    original_present = original.exists() or original.is_symlink()
    fenced_present = fenced.exists() or fenced.is_symlink()
    if original_present and fenced_present:
        raise Refusal(f"both canonical and fenced slot paths exist for {record.slot}")
    if original_present:
        removed = _as_list(journal["removed"], "journal.removed")
        if removed:
            raise StateError("finish journal records removals while the canonical slot still exists")
        salvage = tuple(
            _as_mapping(item, "finish journal.salvage entry")
            for item in _as_list(journal.get("salvage", []), "finish journal.salvage")
        )
        _slot_path, current_checkouts = _reclaim_preconditions(
            config,
            record,
            salvage,
            vcs,
            allow_live_validate_owner=allow_live_validate_owner,
        )
        if current_checkouts != record.checkouts:
            raise Refusal(
                f"slot {record.slot} changed after reclaim was prepared; preserve it and "
                "rerun remove so the changed state is recorded"
            )
        try:
            os.rename(original, fenced)
            _fsync_directory(original.parent)
        except OSError as exc:
            raise Refusal(f"cannot establish path fence {original} -> {fenced}: {exc}") from exc
        _interrupt_for_test("after-path-fence-before-journal")
        fenced_present = True
    if not fenced_present:
        return journal, fenced
    if fenced.is_symlink() or not fenced.is_dir():
        raise Refusal(f"fenced slot is missing or unsafe: {fenced}")
    if original.exists() or original.is_symlink():
        raise Refusal(f"canonical slot still exists after path fence: {original}")
    for checkout in record.checkouts:
        _moved, path = _checkout_at_slot(config, record, checkout, fenced)
        if path.exists() or path.is_symlink():
            _repair_registration_at_slot(config, record, checkout, fenced, vcs)
    journal["phase"] = "fenced"
    _write_journal(config, journal)
    _interrupt_for_test("after-path-fence")
    return journal, fenced


def _finish_remove_paths(
    config: Config,
    record: ActiveRecord,
    journal: dict[str, object],
    vcs: _GitVcs,
    *,
    allow_live_validate_owner: bool = False,
) -> dict[str, object]:
    removed_values = _as_list(journal["removed"], "journal.removed")
    removed = {_as_str(item, "journal.removed item") for item in removed_values}
    all_names = {item.name for item in record.checkouts}
    if not removed <= all_names:
        raise StateError("finish journal names a checkout that is not in the active record")
    journal, fenced_slot = _begin_or_resume_path_fence(
        config,
        record,
        journal,
        vcs,
        allow_live_validate_owner=allow_live_validate_owner,
    )
    present: list[Checkout] = []
    missing_after_remove: list[Checkout] = []
    for checkout in record.checkouts:
        if checkout.name in removed:
            continue
        _moved, path = _checkout_at_slot(config, record, checkout, fenced_slot)
        if path.is_symlink():
            raise Refusal(f"interrupted fenced checkout became a symlink: {path}")
        if path.exists():
            present.append(checkout)
            continue
        _relative, repository = _stored_repository_path(config, checkout.repository)
        canonical = _stored_path(config, checkout.path, "checkout path")
        listed = vcs.listed_worktrees(repository)
        if path.absolute() in listed or canonical.absolute() in listed:
            raise Refusal(
                f"interrupted checkout {checkout.name} is missing but Git still registers it"
            )
        missing_after_remove.append(checkout)
    if missing_after_remove:
        if record.slot_type == "validate":
            for checkout in missing_after_remove:
                _relative, repository = _stored_repository_path(config, checkout.repository)
                vcs.delete_branch_at(repository, checkout.branch, checkout.head)
        removed.update(item.name for item in missing_after_remove)
        journal["removed"] = sorted(removed)
        _write_journal(config, journal)
    remaining = tuple(present)
    if remaining:
        if not fenced_slot.is_dir() or fenced_slot.is_symlink():
            raise Refusal(f"fenced slot is missing or unsafe: {fenced_slot}")
        if _record_layout(config, record) == "nested":
            try:
                actual = {entry.name for entry in fenced_slot.iterdir()}
            except OSError as exc:
                raise Refusal(f"cannot inspect fenced slot {fenced_slot}: {exc}") from exc
            expected = {item.name for item in remaining}
            if record.slot_type == "agent" and (fenced_slot / "HANDOFF.md").is_file():
                expected.add("HANDOFF.md")
            if actual != expected:
                raise Refusal(
                    f"fenced slot contents changed: expected {sorted(expected)}, "
                    f"found {sorted(actual)}"
                )
        elif len(remaining) != 1:
            raise StateError("flat-layout removal has more than one remaining checkout")
        try:
            moved_checkouts: list[Checkout] = []
            moved_paths: dict[str, Path] = {}
            salvage = tuple(
                _as_mapping(item, "finish journal.salvage entry")
                for item in _as_list(journal.get("salvage", []), "finish journal.salvage")
            )
            for checkout in remaining:
                moved, moved_path = _checkout_at_slot(
                    config, record, checkout, fenced_slot
                )
                if record.slot_type == "validate" or salvage:
                    _repair_registration_at_slot(
                        config, record, checkout, fenced_slot, vcs
                    )
                    head = vcs.verify_existing_worktree(
                        _stored_repository_path(config, checkout.repository)[1], moved_path
                    )
                    if head != checkout.head or vcs.branch(moved_path) != checkout.branch:
                        raise Refusal(
                            f"fenced checkout {checkout.name} changed after reclaim was "
                            "prepared; preserve it and rerun remove"
                        )
                else:
                    _repair_checkout_at_slot(
                        config, record, checkout, fenced_slot, vcs
                    )
                moved_checkouts.append(moved)
                moved_paths[checkout.name] = moved_path
            _assert_handoff_read(config, record, fenced_slot)
            if record.slot_type == "agent" and salvage:
                _assert_salvage_still_matches(
                    config, record, salvage, vcs, paths=moved_paths
                )
            _assert_slot_unused(
                fenced_slot,
                None
                if allow_live_validate_owner and record.slot_type == "validate"
                else record,
                use_lsof=not (
                    allow_live_validate_owner and record.slot_type == "validate"
                ),
                ignore_invoking_ancestry=(
                    allow_live_validate_owner and record.slot_type == "validate"
                ),
            )
            for checkout in moved_checkouts:
                for cache in _cache_directories_for_checkout(config, checkout):
                    _remove_cache_directory(config, cache)
        except Refusal as exc:
            if not removed:
                try:
                    _rollback_path_fence(config, record, journal, vcs)
                except Refusal as rollback:
                    raise Refusal(
                        f"{exc}; path-fence rollback failed: {rollback}; run 'wrkslots recover'"
                    ) from rollback
            raise
    elif fenced_slot.exists():
        remaining_entries = {entry.name for entry in fenced_slot.iterdir()}
        allowed = {"HANDOFF.md"} if record.slot_type == "agent" else set()
        unexpected = sorted(remaining_entries - allowed)
        if unexpected:
            raise Refusal(
                f"finished fenced slot contains unexpected entry: {unexpected[0]}"
            )
    for checkout in remaining:
        _relative, repository = _stored_repository_path(config, checkout.repository)
        _moved, path = _checkout_at_slot(config, record, checkout, fenced_slot)
        vcs.remove_worktree(
            repository,
            path,
            force=record.slot_type == "validate" or bool(journal.get("salvage")),
        )
        _interrupt_for_test("after-remove-before-journal")
        if record.slot_type == "validate":
            vcs.delete_branch_at(repository, checkout.branch, checkout.head)
        removed.add(checkout.name)
        journal["removed"] = sorted(removed)
        _write_journal(config, journal)
        _interrupt_for_test("after-remove-worktree")
    if _record_layout(config, record) == "nested" and record.slot_type == "agent":
        handoff = fenced_slot / "HANDOFF.md"
        if handoff.exists() or handoff.is_symlink():
            if handoff.is_symlink() or not handoff.is_file():
                raise Refusal(
                    f"fenced HANDOFF.md became unsafe: {handoff}; preserve the fenced slot "
                    "and run 'wrkslots recover'"
                )
            try:
                contents = handoff.read_bytes()
            except OSError as exc:
                raise Refusal(
                    f"cannot reread fenced HANDOFF.md before removal: {handoff}: {exc}"
                ) from exc
            digest = hashlib.sha256(contents).hexdigest()
            if _recorded_handoff_digest(config, record) != digest:
                raise Refusal(
                    f"fenced HANDOFF.md changed after it was read: {handoff}; preserve the "
                    "slot and rerun 'wrkslots read-handoff' before recovery"
                )
            try:
                handoff.unlink()
                _fsync_directory(fenced_slot)
            except OSError as exc:
                raise Refusal(f"cannot remove recorded HANDOFF.md {handoff}: {exc}") from exc
            _write_event_file(
                config,
                record.machine,
                "handoff-removed",
                {
                    "slot": record.slot,
                    "generation": record.generation,
                    "sha256": digest,
                },
            )
    _remove_fenced_directory(config, fenced_slot)
    journal["phase"] = "removed"
    _write_journal(config, journal)
    return journal


def _assert_physical_slot_removed(
    config: Config,
    record: ActiveRecord,
    vcs: _GitVcs,
    journal: Mapping[str, object],
) -> None:
    slot_path = _slot_directory(config, record.slot, record.slot_type)
    fenced_slot = _finish_fenced_slot(config, record, journal)
    if slot_path.exists() or slot_path.is_symlink():
        raise Refusal(
            f"cannot archive slot {record.slot}: physical slot still exists at {slot_path}"
        )
    if fenced_slot.exists() or fenced_slot.is_symlink():
        raise Refusal(
            f"cannot archive slot {record.slot}: fenced storage still exists at {fenced_slot}"
        )
    for checkout in record.checkouts:
        path = _stored_path(config, checkout.path, "checkout path")
        if path.exists() or path.is_symlink():
            raise Refusal(
                f"cannot archive slot {record.slot}: checkout still exists at {path}"
            )
        _relative, repository = _stored_repository_path(config, checkout.repository)
        _moved, fenced_path = _checkout_at_slot(
            config, record, checkout, fenced_slot
        )
        listed = vcs.listed_worktrees(repository)
        if path.absolute() in listed or fenced_path.absolute() in listed:
            raise Refusal(
                f"cannot archive slot {record.slot}: Git still registers checkout {checkout.name}"
            )


def _finish_state_update(
    config: Config,
    state: ActiveState,
    record: ActiveRecord,
    journal: Mapping[str, object],
) -> None:
    current_slots = {item.slot: item for item in state.slots}
    current = current_slots.get(record.slot)
    if current is not None and _record_to_obj(current) != _record_to_obj(record):
        raise StateError("active record changed during an interrupted finish")
    _assert_physical_slot_removed(config, record, _GitVcs(), journal)
    archive = _load_archive(config)
    entry = _archive_entry(journal, record)
    _append_archive_once(config, archive, entry)
    _interrupt_for_test("after-archive-before-active")
    if current is not None:
        updated_state = _delete_record(state, record.slot)
        _write_active_state(
            config,
            updated_state,
            action="slot-removed",
            slot=record.slot,
            evidence={"archive_id": entry["archive_id"]},
        )
    _clear_journal(config, journal)


def _begin_finish(
    config: Config,
    state: ActiveState,
    record: ActiveRecord,
    *,
    mode: str,
    actor: str,
    salvage: Sequence[Mapping[str, object]] = (),
    validate_complete: bool = False,
    allow_live_validate_owner: bool = False,
) -> None:
    vcs = _GitVcs()
    _load_archive(config)
    _assert_not_held(config, record)
    _slot_path, final_checkouts = _reclaim_preconditions(
        config,
        record,
        salvage,
        vcs,
        allow_live_validate_owner=allow_live_validate_owner,
    )
    final_record = dataclasses.replace(record, checkouts=final_checkouts)
    state = _replace_record(state, final_record)
    _write_active_state(
        config,
        state,
        action="checkout-evidence-refreshed",
        slot=record.slot,
    )
    finished_at = _utc_now()
    journal = _finish_journal_payload(
        config,
        final_record,
        mode=mode,
        actor=actor,
        finished_at=finished_at,
        removed=(),
        phase="prepared",
        salvage=salvage,
        validate_complete=validate_complete,
    )
    _write_journal(config, journal)
    _interrupt_for_test("after-finish-journal")
    try:
        journal = _finish_remove_paths(
            config,
            final_record,
            journal,
            vcs,
            allow_live_validate_owner=allow_live_validate_owner,
        )
    except Refusal:
        canonical = _slot_directory(config, final_record.slot, final_record.slot_type)
        fenced = _finish_fenced_slot(config, final_record, journal)
        if (
            canonical.is_dir()
            and not canonical.is_symlink()
            and not fenced.exists()
            and not fenced.is_symlink()
            and not _as_list(journal["removed"], "journal.removed")
        ):
            _clear_journal(config, journal)
        raise
    _finish_state_update(config, state, final_record, journal)


def _cmd_finish(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    _validate_name(args.slot, "slot")
    _validate_name(args.agent, "agent")
    owner = _capture_caller_process(args.owner_pid, "owner")
    validation = tuple(args.validation or ())
    limitations = tuple(args.limitation or ())
    if not validation or any(not item.strip() for item in validation):
        raise Refusal("finish requires at least one non-empty --validation entry")
    if any(not item.strip() for item in limitations):
        raise Refusal("finish limitations must be non-empty")
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        before = _global_rows(states, archives)
        state = _load_active(config)
        record = _find_record(state, args.slot)
        _assert_owner_auth(record, args.agent, owner, args.expected_generation)
        if record.handoff is not None:
            raise Refusal(f"slot {record.slot} already has a recorded handoff")
        _slot_path, final_checkouts = _handoff_preconditions(
            config, record, _GitVcs()
        )
        continuation = (
            f"wrkslots remove {record.slot} --coordinator-pid "
            f"<current-coordinator-pid> --expected-generation {record.generation}"
        )
        updated = dataclasses.replace(
            record,
            checkouts=final_checkouts,
            handoff=Handoff(
                recorded_at=_utc_now(),
                validation=validation,
                limitations=limitations,
                continuation=continuation,
            ),
        )
        _write_active_state(
            config,
            _replace_record(state, updated),
            action="handoff-recorded",
            slot=updated.slot,
            evidence={"validation": list(validation)},
        )
        _interrupt_for_test("after-handoff-write")
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    print(
        f"recorded handoff slot={record.slot} generation={record.generation}; "
        "physical storage retained"
    )
    print(f"continuation={continuation}")
    return 0


def _cmd_remove(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    _validate_name(args.slot, "slot")
    coordinator = _capture_caller_process(args.coordinator_pid, "coordinator")
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_command_registry_storage(config, states, args)
        before = _global_rows(states, archives)
        state = _load_active(config)
        record = _find_record(state, args.slot)
        _expected_generation(record, args.expected_generation)
        _assert_caller_process(coordinator, "coordinator")
        if args.validate_complete and record.slot_type != "validate":
            raise Refusal(
                "--validate-complete applies only to a slot created with --slot-type validate. "
                "state: REFUSED -- no checkout was salvaged or removed. remedy: omit the flag "
                "and satisfy the ordinary agent-slot reclaim conditions"
            )
        _assert_not_held(config, record)
        age, expired = _heartbeat_diagnosis(record)
        owner_state, detail = _process_state(record.owner)
        live_validate_owner = bool(
            args.validate_complete
            and owner_state == "live"
            and record.owner == coordinator
        )
        if live_validate_owner:
            _assert_caller_process(coordinator, "validate owner")
        else:
            _assert_registered_liveness(config, record)
        if not expired and not args.validate_complete:
            remaining = max(1, record.heartbeat_ttl_seconds - int(age))
            raise Refusal(
                f"slot {record.slot} heartbeat is {int(age)}s old and its "
                f"{record.heartbeat_ttl_seconds}s time-to-live has not expired. state: "
                "REFUSED -- no checkout was salvaged or removed. remedy: wait at least "
                f"{remaining}s without renewal before retrying remove; if the owner is still "
                "working, have it run 'wrkslots heartbeat' instead"
            )
        if record.owner is None:
            raise Refusal(
                f"slot {record.slot} has no recorded owner process, so owner death is unknown. "
                "state: REFUSED -- no checkout was salvaged or removed. remedy: preserve this "
                "slot until an evidence-based migration can establish its historical owner; "
                "heartbeat expiry alone cannot do that"
            )
        if owner_state != "dead" and not live_validate_owner:
            raise Refusal(
                f"remove requires a proven-dead recorded owner; owner is {owner_state}: {detail}. "
                "state: REFUSED -- no checkout was salvaged or removed. remedy: wait for the "
                "exact owner process to exit and for the time-to-live to remain expired; if "
                "process evidence is unavailable, repair that evidence rather than treating the "
                "slot as free"
            )
        slot_path = _assert_slot_contents(config, record)
        _assert_handoff_read(config, record, slot_path)
        _assert_slot_unused(
            slot_path,
            None if live_validate_owner else record,
            use_lsof=not live_validate_owner,
            ignore_invoking_ancestry=live_validate_owner,
        )
        _ensure_event_log(config, record.machine)
        _write_event_file(
            config,
            record.machine,
            "reclaim-started",
            {
                "slot": record.slot,
                "generation": record.generation,
                "actor": _identity_to_obj(coordinator),
                "coordinator_authorized": bool(args.coordinator_authorized),
                "owner_state": owner_state,
                "registered_liveness": "dead",
                "heartbeat_age_seconds": int(age),
                "heartbeat_ttl_seconds": record.heartbeat_ttl_seconds,
                "validate_complete": bool(args.validate_complete),
                "live_validate_owner": live_validate_owner,
            },
        )
        salvage: tuple[dict[str, object], ...]
        evidence: tuple[str, ...]
        if record.slot_type == "validate":
            salvage = ()
            evidence = (
                "slot type validate: authored work is excluded by construction, so salvage was not run",
            )
        elif not record.checkouts and record.import_source is not None:
            salvage = ()
            evidence = (
                "historical agent slot contains no checkout and its exact contents were "
                "verified before deletion, so there was no checkout to salvage",
            )
        else:
            salvage = _salvage_agent_slot(config, record, _GitVcs())
            evidence = tuple(
                f"checkout {item['checkout']}: {item['disposition']} commit "
                f"{item['salvage_commit']}"
                + (
                    f" at {item['remote_ref']}"
                    if item.get("remote_ref") is not None
                    else " on an existing remote ref"
                )
                for item in salvage
            )
        if record.handoff is None:
            continuation = (
                f"wrkslots remove {record.slot} --coordinator-pid <current-coordinator-pid> "
                f"--expected-generation {record.generation}"
            )
            record = dataclasses.replace(
                record,
                handoff=Handoff(
                    recorded_at=_utc_now(),
                    validation=evidence,
                    limitations=(),
                    continuation=continuation,
                ),
            )
            state = _replace_record(state, record)
            _write_active_state(
                config,
                state,
                action="reclaim-evidence-recorded",
                slot=record.slot,
                evidence={
                    "actor": _identity_to_obj(coordinator),
                    "coordinator_authorized": bool(args.coordinator_authorized),
                    "owner_state": owner_state,
                    "heartbeat_age_seconds": int(age),
                    "salvage": list(salvage),
                },
            )
        _begin_finish(
            config,
            state,
            record,
            mode="remove",
            actor="coordinator",
            salvage=salvage,
            validate_complete=bool(args.validate_complete),
            allow_live_validate_owner=live_validate_owner,
        )
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    print(f"removed and archived slot={record.slot} generation={record.generation}")
    return 0


def _recover_partial_updates(config: Config, discard: bool) -> bool:
    leftovers = sorted(config.control.glob("ACTIVE.*.json.tmp.*"))
    leftovers += sorted(config.control.glob("ARCHIVED.*.json.tmp.*"))
    leftovers += sorted(config.control.glob("ACTIVE.*.journal.tmp.*"))
    for event_directory in sorted(config.control.glob("EVENTS.*")):
        if event_directory.is_dir() and not event_directory.is_symlink():
            leftovers += sorted(event_directory.glob("*.json.tmp.*"))
    if not leftovers:
        return False
    if not discard:
        raise StateError(
            f"partial atomic update found: {leftovers[0]}; inspect it, then rerun recover "
            "with --discard-partial to keep the last durable state"
        )
    for leftover in leftovers:
        if leftover.is_symlink() or not leftover.is_file():
            raise StateError(f"partial state path is not a regular file: {leftover}")
        marker = ".tmp."
        target_name, separator, _suffix = leftover.name.partition(marker)
        if not separator:
            raise StateError(f"unrecognized partial state filename: {leftover}")
        target = leftover.parent / target_name
        event_directory_match = re.fullmatch(
            r"EVENTS\.([A-Za-z0-9][A-Za-z0-9._-]{0,63})", leftover.parent.name
        )
        event_name_match = re.fullmatch(r"([0-9]{20})\.json", target.name)
        if event_directory_match is not None and event_name_match is not None:
            machine = event_directory_match.group(1)
            same_target = sorted(leftover.parent.glob(f"{target.name}.tmp.*"))
            if len(same_target) != 1:
                raise StateError(
                    f"cannot recover append-only event {target.name}: multiple temp files exist"
                )
            durable_paths = sorted(
                entry
                for entry in leftover.parent.iterdir()
                if re.fullmatch(r"[0-9]{20}\.json", entry.name)
            )
            durable_events = _load_event_paths(durable_paths, machine)
            if target.exists() or target.is_symlink():
                if target.is_symlink() or not target.is_file():
                    raise StateError(
                        f"cannot discard {leftover}: durable event is unsafe: {target}"
                    )
                leftover.unlink()
                _fsync_directory(leftover.parent)
                print(
                    f"discarded incomplete append-only event {leftover.name}; "
                    f"kept durable {target.name}"
                )
                continue
            expected_sequence = len(durable_events) + 1
            if int(event_name_match.group(1)) != expected_sequence:
                raise StateError(
                    f"cannot recover append-only event {target.name}: expected sequence "
                    f"{expected_sequence:020d}"
                )
            previous = (
                "0" * 64
                if not durable_events
                else _as_str(
                    durable_events[-1]["sha256"], "append-only event.sha256"
                )
            )
            _event_from_path(
                leftover,
                machine,
                expected_sequence,
                previous,
                require_filename=False,
            )
            os.replace(leftover, target)
            _fsync_directory(target.parent)
            _load_events(config, machine)
            print(f"recovered append-only event {target.name} from {leftover.name}")
            continue
        active_match = re.fullmatch(
            r"ACTIVE\.([A-Za-z0-9][A-Za-z0-9._-]{0,63})\.json", target.name
        )
        archive_match = re.fullmatch(
            r"ARCHIVED\.([A-Za-z0-9][A-Za-z0-9._-]{0,63})\.json", target.name
        )
        if not target.exists() and not target.is_symlink():
            matches = sorted(leftover.parent.glob(f"{target.name}.tmp.*"))
            if len(matches) != 1:
                raise StateError(
                    f"cannot recover initial state for {target.name}: multiple temp files exist"
                )
            raw = _as_mapping(_read_json(leftover, "initial state temp file"), "initial state temp file")
            machine_match = active_match or archive_match
            if machine_match is None:
                raise StateError(
                    f"cannot promote temp file without a durable target: {leftover}"
                )
            if (
                _as_int(raw.get("schema"), "initial state schema") != SCHEMA
                or _as_str(raw.get("machine"), "initial state machine")
                != machine_match.group(1)
            ):
                raise StateError(f"initial state temp file is invalid: {leftover}")
            os.replace(leftover, target)
            _fsync_directory(target.parent)
            if active_match is not None:
                _load_active(config, active_match.group(1))
            else:
                assert archive_match is not None
                _load_archive(config, archive_match.group(1))
            print(f"recovered initial state {target.name} from {leftover.name}")
            continue
        if not target.is_file() or target.is_symlink():
            raise StateError(
                f"cannot discard {leftover}: last durable state is unsafe: {target}"
            )
        if active_match is not None:
            machine = active_match.group(1)
            state = _load_active(config, machine)
            if _load_events(config, machine):
                leftover.unlink()
                _fsync_directory(leftover.parent)
                _atomic_write_json(target, _active_to_obj(state))
                print(
                    f"discarded incomplete atomic update {leftover.name}; refreshed "
                    f"{target.name} from the append-only log"
                )
                continue
        elif archive_match is not None:
            machine = archive_match.group(1)
            archive_state = _load_archive(config, machine)
            if _load_events(config, machine):
                leftover.unlink()
                _fsync_directory(leftover.parent)
                _atomic_write_json(target, _archive_to_obj(archive_state))
                print(
                    f"discarded incomplete atomic update {leftover.name}; refreshed "
                    f"{target.name} from the append-only log"
                )
                continue
        elif target.name.endswith(".journal"):
            raise StateError(
                f"cannot automatically discard journal update {leftover}; "
                "preserve both files for explicit inspection"
            )
        else:
            raise StateError(f"unrecognized durable state target: {target}")
        leftover.unlink()
        _fsync_directory(leftover.parent)
        print(f"discarded incomplete atomic update {leftover.name}; kept {target.name}")
    return True


def _abort_create(
    config: Config,
    journal_path: Path,
    slot: str,
    slot_type: str,
    plan: Sequence[PlannedCheckout],
    recorded_created: Sequence[Checkout],
) -> None:
    slot_path = _slot_directory(config, slot, slot_type)
    vcs = _GitVcs()
    recorded_by_name = {checkout.name: checkout for checkout in recorded_created}
    if config.layout == "nested" and slot_path.exists():
        if not slot_path.is_dir() or slot_path.is_symlink():
            raise Refusal(f"cannot abort unsafe slot path: {slot_path}")
        expected_entries = {
            item.name
            for item in plan
            if _stored_path(config, item.destination, "checkout destination").exists()
        }
        actual_entries = {entry.name for entry in slot_path.iterdir()}
        if actual_entries != expected_entries:
            raise Refusal(
                f"cannot abort slot with unexpected root contents: expected "
                f"{sorted(expected_entries)}, found {sorted(actual_entries)}"
            )
    preflight: list[
        tuple[
            PlannedCheckout,
            Path,
            Path,
            str,
            bool,
            Checkout | None,
            tuple[CacheDirectory, ...],
        ]
    ] = []
    for item in plan:
        _relative, repository = _stored_repository_path(config, item.repository)
        destination = _stored_path(config, item.destination, "checkout destination")
        recorded = recorded_by_name.get(item.name)
        expected_head = item.start_point if recorded is None else recorded.head
        if destination.exists() or destination.is_symlink():
            if destination.is_symlink() or not destination.is_dir():
                raise Refusal(f"cannot abort unsafe checkout path: {destination}")
            head = vcs.verify_existing_worktree(repository, destination)
            branch = vcs.branch(destination)
            if head != expected_head or branch != item.branch:
                raise Refusal(
                    f"cannot abort changed checkout {item.name}: expected branch "
                    f"{item.branch} at {expected_head}, found {branch} at {head}"
                )
            operation_paths = vcs.operation_paths(destination)
            if operation_paths:
                raise Refusal(
                    f"cannot abort checkout {item.name} with an unfinished Git operation: "
                    f"{operation_paths[0]}"
                )
            vcs.assert_ordinary_history(destination)
            vcs.assert_ordinary_index(destination)
            checkout = recorded or Checkout(
                name=item.name,
                path=item.destination,
                repository=item.repository,
                branch=item.branch,
                start_point=item.start_point,
                remote=item.remote,
                remote_url_sha256=item.remote_url_sha256,
                landed_ref=item.landed_ref,
                head=expected_head,
            )
            _assert_cache_policy_untracked(config, checkout, vcs)
            status = vcs.status(destination, _cache_globs_for(config, checkout.name))
            if status:
                first = status.splitlines()[0]
                raise Refusal(
                    f"cannot abort changed checkout {item.name}: source state is dirty "
                    f"({first}); preserve it for inspection"
                )
            present = True
            caches = _cache_directories_for_checkout(config, checkout)
        elif destination.absolute() in vcs.listed_worktrees(repository):
            raise Refusal(
                f"cannot abort missing checkout still registered by Git: {destination}"
            )
        else:
            present = False
            checkout = None
            caches = ()
        if vcs.branch_exists(repository, item.branch):
            branch_head = vcs.verify_ref(
                repository, f"refs/heads/{item.branch}", "created branch"
            )
            if branch_head != expected_head:
                raise Refusal(
                    f"created branch {item.branch} moved from {expected_head} to "
                    f"{branch_head}; preserve it instead of aborting provisioning"
                )
        preflight.append(
            (item, repository, destination, expected_head, present, checkout, caches)
        )
    if slot_path.exists():
        _assert_slot_unused(slot_path)
    for item, repository, destination, expected_head, present, checkout, caches in preflight:
        if present:
            assert checkout is not None
            head = vcs.verify_existing_worktree(repository, destination)
            branch = vcs.branch(destination)
            if head != expected_head or branch != item.branch:
                raise Refusal(
                    f"cannot abort changed checkout {item.name}: expected branch "
                    f"{item.branch} at {expected_head}, found {branch} at {head}"
                )
            operation_paths = vcs.operation_paths(destination)
            if operation_paths:
                raise Refusal(
                    f"cannot abort checkout {item.name} with an unfinished Git operation: "
                    f"{operation_paths[0]}"
                )
            vcs.assert_ordinary_history(destination)
            vcs.assert_ordinary_index(destination)
            _assert_cache_policy_untracked(config, checkout, vcs)
            status = vcs.status(destination, _cache_globs_for(config, checkout.name))
            if status:
                first = status.splitlines()[0]
                raise Refusal(
                    f"cannot abort changed checkout {item.name}: source state is dirty "
                    f"({first}); preserve it for inspection"
                )
            for cache in caches:
                _remove_cache_directory(config, cache)
            vcs.remove_worktree(repository, destination)
            _fsync_directory(slot_path.parent)
    for item, repository, _destination, expected_head, _present, _checkout, _caches in preflight:
        vcs.delete_branch_at(repository, item.branch, expected_head)
    if slot_path.exists():
        try:
            slot_path.rmdir()
        except OSError as exc:
            raise Refusal(f"cannot remove aborted slot directory {slot_path}: {exc}") from exc
        _fsync_directory(slot_path.parent)
    _clear_journal(
        config,
        _as_mapping(_read_json(journal_path, "create journal"), "create journal"),
    )
    print(f"aborted incomplete create slot={slot}; removed provisional worktrees and branches")


def _recover_create(
    config: Config,
    path: Path,
    raw: Mapping[str, object],
    state: ActiveState,
    recovery_actor: ProcessIdentity,
    *,
    retry_running_hook: bool,
    abort_create: bool,
) -> None:
    required = {
        "schema",
        "kind",
        "machine",
        "slot",
        "agent",
        "task",
        "purpose",
        "created_at",
        "heartbeat_at",
        "heartbeat_ttl_seconds",
        "owner",
        "coordinator_lease",
        "planned",
        "created",
    }
    optional = {
        "slot_type",
        "post_provision_hooks",
        "hook_progress",
        "hook_failure",
        "failure_policy",
    }
    _exact_keys(raw, required, optional, "create journal")
    machine = _as_str(raw["machine"], "create journal.machine")
    if machine != config.machine or path != _journal_path(config, machine):
        raise StateError(
            f"create journal filename does not match machine {config.machine}"
        )
    slot = _as_str(raw["slot"], "create journal.slot")
    agent = _as_str(raw["agent"], "create journal.agent")
    _validate_name(slot, "slot")
    _validate_name(agent, "agent")
    slot_type = _as_str(raw.get("slot_type", "agent"), "create journal.slot_type")
    if slot_type not in SLOT_TYPES:
        raise StateError(
            f"create journal.slot_type must be one of {', '.join(SLOT_TYPES)}"
        )
    plan = tuple(
        _planned_from_obj(item, f"create journal.planned[{index}]")
        for index, item in enumerate(
            _as_list(raw["planned"], "create journal.planned")
        )
    )
    recorded_created = tuple(
        _checkout_from_obj(item, f"create journal.created[{index}]")
        for index, item in enumerate(
            _as_list(raw["created"], "create journal.created")
        )
    )
    journal_hooks = (
        ()
        if "post_provision_hooks" not in raw
        else _string_tuple(
            raw["post_provision_hooks"],
            "create journal.post_provision_hooks",
            _validate_hook,
            unique=False,
        )
    )
    if journal_hooks != config.post_provision_hooks:
        raise StateError("create journal post-provision hooks differ from configuration")
    hook_progress = _as_int(raw.get("hook_progress", 0), "create journal.hook_progress")
    total_hook_steps = len(plan) * len(journal_hooks)
    if hook_progress > total_hook_steps:
        raise StateError("create journal hook progress exceeds its hook plan")
    if "failure_policy" in raw and _as_str(
        raw["failure_policy"], "create journal.failure_policy"
    ) != "leave-for-inspection":
        raise StateError("create journal has an unsupported hook failure policy")
    hook_failure = raw.get("hook_failure")
    failure: Mapping[str, object] | None = None
    hook_failure_status: str | None = None
    if hook_failure is not None:
        failure = _as_mapping(hook_failure, "create journal.hook_failure")
        _exact_keys(
            failure,
            {"checkout", "hook_index", "command", "status"},
            {"returncode", "detail"},
            "create journal.hook_failure",
        )
        hook_failure_status = _as_str(
            failure["status"], "create journal.hook_failure.status"
        )
        if hook_failure_status not in {"running", "failed", "failed-to-start"}:
            raise StateError("create journal has an unknown hook failure status")
    if not plan:
        raise StateError("create journal contains no planned checkouts")
    if len({item.name for item in plan}) != len(plan):
        raise StateError("create journal has duplicate planned checkout names")
    if len({item.name for item in recorded_created}) != len(recorded_created):
        raise StateError("create journal has duplicate created checkout names")
    if [item.name for item in recorded_created] != [
        item.name for item in plan[: len(recorded_created)]
    ]:
        raise StateError("create journal's completed checkouts are not a planned prefix")
    if hook_progress > len(recorded_created) * len(journal_hooks):
        raise StateError("create journal ran hooks for a checkout not durably recorded")
    if failure is not None:
        if not journal_hooks or hook_progress >= total_hook_steps:
            raise StateError("create journal records a failed hook after hook completion")
        if len(recorded_created) != len(plan):
            raise StateError("create journal records a hook before every checkout was created")
        expected_checkout = plan[hook_progress // len(journal_hooks)].name
        expected_hook_index = hook_progress % len(journal_hooks)
        failure_checkout = _as_str(
            failure["checkout"], "create journal.hook_failure.checkout"
        )
        failure_hook_index = _as_int(
            failure["hook_index"], "create journal.hook_failure.hook_index"
        )
        failure_command = _as_str(
            failure["command"], "create journal.hook_failure.command"
        )
        if (
            failure_checkout != expected_checkout
            or failure_hook_index != expected_hook_index
            or failure_command != journal_hooks[expected_hook_index]
        ):
            raise StateError("create journal hook failure does not match its progress")
        if hook_failure_status == "running":
            if "returncode" in failure or "detail" in failure:
                raise StateError("running create hook records impossible completion details")
            if not retry_running_hook and not abort_create:
                raise Refusal(
                    "the create journal records a hook that was running when provisioning "
                    "stopped; it may already have completed. Inspect its effects, then rerun "
                    "recover with --retry-running-hook to authorize another execution"
                )
        elif hook_failure_status == "failed":
            returncode = failure.get("returncode")
            if (
                not isinstance(returncode, int)
                or isinstance(returncode, bool)
                or returncode == 0
                or "detail" in failure
            ):
                raise StateError("failed create hook has invalid completion details")
        elif (
            "returncode" in failure
            or not _as_str(
                failure.get("detail"), "create journal.hook_failure.detail"
            ).strip()
        ):
            raise StateError("failed-to-start create hook has invalid completion details")
    planned_by_name = {item.name: item for item in plan}
    for item in plan:
        expected_destination, _destination = _checkout_path(
            config, slot, item.name, slot_type
        )
        if item.destination != expected_destination:
            raise StateError(
                f"create journal destination for {item.name} does not match slot {slot}"
            )
        normalized_repository, _repository = _stored_repository_path(
            config, item.repository
        )
        if (
            not Path(item.repository).is_absolute()
            and item.repository != normalized_repository
        ):
            raise StateError(
                f"create journal repository for {item.name} is not canonical"
            )
        if item.landed_ref != _landed_ref_for_remote(config, item.remote):
            raise StateError(
                f"create journal target for {item.name} differs from configured authority"
            )
        if _GitVcs().remote_url_sha256(_repository, item.remote) != item.remote_url_sha256:
            raise Refusal(
                f"create journal remote {item.remote!r} URL changed for {item.name}"
            )
        if not SHA_RE.fullmatch(item.start_point):
            raise StateError(
                f"create journal start point for {item.name} is not an exact commit"
            )
    for checkout in recorded_created:
        planned = planned_by_name.get(checkout.name)
        if planned is None:
            raise StateError("create journal records an unplanned checkout")
        expected_fields = (
            planned.destination,
            planned.repository,
            planned.branch,
            planned.start_point,
            planned.remote,
            planned.remote_url_sha256,
            planned.landed_ref,
        )
        actual_fields = (
            checkout.path,
            checkout.repository,
            checkout.branch,
            checkout.start_point,
            checkout.remote,
            checkout.remote_url_sha256,
            checkout.landed_ref,
        )
        if actual_fields != expected_fields or checkout.head != planned.start_point:
            raise StateError(
                f"create journal's completed checkout {checkout.name} differs from its plan"
            )
    task = _as_str(raw["task"], "create journal.task")
    purpose = _as_str(raw["purpose"], "create journal.purpose")
    if not task or not purpose:
        raise StateError("create journal has an empty task or purpose")
    created_at = _as_str(raw["created_at"], "create journal.created_at")
    _parse_timestamp(created_at, "create journal.created_at")
    heartbeat_at = _as_str(raw["heartbeat_at"], "create journal.heartbeat_at")
    _parse_timestamp(heartbeat_at, "create journal.heartbeat_at")
    heartbeat_ttl_seconds = _as_int(
        raw["heartbeat_ttl_seconds"],
        "create journal.heartbeat_ttl_seconds",
        minimum=1,
    )
    if heartbeat_ttl_seconds != config.heartbeat_ttl_seconds:
        raise StateError("create journal heartbeat TTL differs from configuration")
    owner = _identity_from_obj(raw["owner"], "create journal.owner")
    coordinator_lease = _identity_from_obj(
        raw["coordinator_lease"], "create journal.coordinator_lease"
    )
    if coordinator_lease is None:
        raise StateError("create journal has no coordinator lease")
    existing_record = next((item for item in state.slots if item.slot == slot), None)
    if existing_record is not None:
        if abort_create:
            raise Refusal(
                f"cannot abort create for slot {slot}: its ACTIVE row is already durable"
            )
        if hook_progress != total_hook_steps:
            raise StateError(
                "create journal reached ACTIVE publication before post-provision hooks completed"
            )
        expected_record = ActiveRecord(
            slot=slot,
            agent=agent,
            task=task,
            purpose=purpose,
            slot_type=slot_type,
            machine=config.machine,
            generation=1,
            created_at=created_at,
            heartbeat_at=heartbeat_at,
            heartbeat_ttl_seconds=heartbeat_ttl_seconds,
            owner=owner,
            coordinator_lease=coordinator_lease,
            coordinator_recovery_note=None,
            handoff=None,
            checkouts=recorded_created,
        )
        if len(recorded_created) != len(plan) or _record_to_obj(
            existing_record
        ) != _record_to_obj(expected_record):
            raise StateError("create journal does not exactly match its durable ACTIVE record")
        _assert_record_paths(config, existing_record)
        for checkout in existing_record.checkouts:
            _relative, repository = _stored_repository_path(config, checkout.repository)
            destination = _stored_path(config, checkout.path, "checkout path")
            head = _GitVcs().verify_existing_worktree(repository, destination)
            if head != checkout.head:
                raise Refusal(
                    f"create recovery found changed HEAD for {checkout.name}; preserve it for inspection"
                )
        _clear_journal(config, raw)
        print(f"recovered create: active state was durable; cleared journal for {slot}")
        return
    if abort_create:
        _abort_create(config, path, slot, slot_type, plan, recorded_created)
        return
    _assert_agent_and_slot_free(config, slot, agent, slot_type)
    slot_path = _slot_directory(config, slot, slot_type)
    if slot_path.exists() and (not slot_path.is_dir() or slot_path.is_symlink()):
        raise Refusal(f"create recovery found an unsafe slot path: {slot_path}")
    if not slot_path.exists() and config.layout == "nested":
        slot_path.mkdir(mode=0o755)
        _fsync_directory(slot_path.parent)
    vcs = _GitVcs()
    created: list[Checkout] = []
    for item in plan:
        destination = _stored_path(config, item.destination, "checkout destination")
        _relative, repository = _stored_repository_path(config, item.repository)
        if destination.exists() or destination.is_symlink():
            if destination.is_symlink() or not destination.is_dir():
                raise Refusal(f"create recovery found an unsafe destination: {destination}")
            head = vcs.verify_existing_worktree(repository, destination)
            branch = vcs.branch(destination)
            if branch != item.branch:
                raise Refusal(
                    f"create recovery branch mismatch for {item.name}: "
                    f"expected {item.branch}, found {branch}"
                )
            recorded = next(
                (value for value in recorded_created if value.name == item.name), None
            )
            expected_head = recorded.head if recorded is not None else item.start_point
            if head != expected_head:
                raise Refusal(
                    f"create recovery found changed HEAD for {item.name}; preserve it for inspection"
                )
        else:
            if vcs.branch_exists(repository, item.branch):
                raise Refusal(
                    f"create recovery found branch {item.branch} without its registered path; "
                    "preserve it for inspection"
                )
            head = vcs.add_worktree(repository, destination, item.branch, item.start_point)
        checkout = Checkout(
            name=item.name,
            path=item.destination,
            repository=item.repository,
            branch=item.branch,
            start_point=item.start_point,
            remote=item.remote,
            remote_url_sha256=item.remote_url_sha256,
            landed_ref=item.landed_ref,
            head=head,
        )
        created.append(checkout)
        updated_journal = dict(raw)
        updated_journal["created"] = [_checkout_to_obj(value) for value in created]
        _write_journal(config, updated_journal)
    if config.layout == "nested":
        actual = {entry.name for entry in slot_path.iterdir()}
        expected = {item.name for item in plan}
        if actual != expected:
            raise Refusal(
                f"create recovery found unexpected slot contents: expected {sorted(expected)}, "
                f"found {sorted(actual)}"
            )
    updated_journal = dict(raw)
    updated_journal["created"] = [_checkout_to_obj(value) for value in created]
    updated_journal["post_provision_hooks"] = list(journal_hooks)
    updated_journal["hook_progress"] = hook_progress
    updated_journal["hook_failure"] = None
    updated_journal["failure_policy"] = "leave-for-inspection"
    _write_journal(config, updated_journal)
    hook_progress = _run_post_provision_hooks(
        config,
        created,
        path,
        updated_journal,
        start=hook_progress,
    )
    if hook_progress != total_hook_steps:
        raise StateError("create recovery did not complete its post-provision hooks")
    for checkout in created:
        _assert_checkout_identity_unchanged(config, checkout, vcs)
    record = ActiveRecord(
        slot=slot,
        agent=agent,
        task=task,
        purpose=purpose,
        slot_type=slot_type,
        machine=config.machine,
        generation=1,
        created_at=created_at,
        heartbeat_at=heartbeat_at,
        heartbeat_ttl_seconds=heartbeat_ttl_seconds,
        owner=owner,
        coordinator_lease=coordinator_lease,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=tuple(created),
    )
    _write_active_state(
        config,
        _append_record(state, record),
        action="slot-created-by-recovery",
        slot=record.slot,
        evidence={
            "slot_type": record.slot_type,
            "original_coordinator": _identity_to_obj(coordinator_lease),
            "recovery_actor": _identity_to_obj(recovery_actor),
        },
    )
    _clear_journal(config, updated_journal)
    print(f"recovered create: registered slot={slot} checkouts={len(created)}")


def _recover_import_existing(
    config: Config,
    path: Path,
    raw: Mapping[str, object],
    state: ActiveState,
    recovery_actor: ProcessIdentity,
) -> None:
    _exact_keys(
        raw,
        {"schema", "kind", "machine", "slot", "record"},
        set(),
        "import journal",
    )
    machine = _as_str(raw["machine"], "import journal.machine")
    if machine != config.machine or path != _journal_path(config, machine):
        raise StateError(
            f"import journal filename does not match machine {config.machine}"
        )
    slot = _validate_name(_as_str(raw["slot"], "import journal.slot"), "slot")
    record = _record_from_obj(raw["record"], "import journal.record")
    if record.slot != slot or record.machine != machine:
        raise StateError("import journal record does not match its shard or slot")
    existing = next((item for item in state.slots if item.slot == slot), None)
    if existing is not None:
        if _record_to_obj(existing) != _record_to_obj(record):
            raise StateError(
                f"import journal for slot {slot} does not match its durable ACTIVE row"
            )
        _verify_import_record(config, existing, _GitVcs())
        _clear_journal(config, raw)
        print(f"recovered import: active state was durable; cleared journal for {slot}")
        return
    _assert_agent_and_slot_free(
        config, slot, record.agent, record.slot_type, enforce_cap=False
    )
    _verify_import_record(config, record, _GitVcs())
    _write_active_state(
        config,
        _append_record(state, record),
        action="slot-imported-by-recovery",
        slot=slot,
        evidence={
            "slot_type": record.slot_type,
            "original_coordinator": _identity_to_obj(record.coordinator_lease),
            "recovery_actor": _identity_to_obj(recovery_actor),
            "import_source": _import_source_to_obj(record.import_source),
        },
    )
    _clear_journal(config, raw)
    print(f"recovered import: registered slot={slot} checkouts={len(record.checkouts)}")


def _recover_finish(
    config: Config,
    path: Path,
    raw: Mapping[str, object],
    state: ActiveState,
    coordinator: ProcessIdentity,
) -> None:
    required = {
        "schema",
        "kind",
        "machine",
        "slot",
        "mode",
        "actor",
        "finished_at",
        "archive_id",
        "phase",
        "fenced",
        "removed",
        "record",
    }
    _exact_keys(raw, required, {"salvage", "validate_complete"}, "finish journal")
    if path != _journal_path(config):
        raise StateError(f"finish journal filename does not match machine {config.machine}")
    record = _record_from_obj(raw["record"], "finish journal.record")
    if record.machine != config.machine:
        raise StateError("finish journal record belongs to a different machine")
    journal_slot = _as_str(raw["slot"], "finish journal.slot")
    if journal_slot != record.slot:
        raise StateError("finish journal slot does not match its recorded active state")
    finished_at = _as_str(raw["finished_at"], "finish journal.finished_at")
    _parse_timestamp(finished_at, "finish journal.finished_at")
    expected_archive_id = (
        f"{config.machine}:{record.slot}:{record.generation}:{finished_at}"
    )
    if _as_str(raw["archive_id"], "finish journal.archive_id") != expected_archive_id:
        raise StateError("finish journal archive_id does not match its record")
    mode = _as_str(raw["mode"], "finish journal.mode")
    actor = _as_str(raw["actor"], "finish journal.actor")
    if mode != "remove" or actor != "coordinator":
        raise StateError("finish journal actor does not match its operation")
    removed_values = _as_list(raw["removed"], "finish journal.removed")
    removed_names = [
        _as_str(value, f"finish journal.removed[{index}]")
        for index, value in enumerate(removed_values)
    ]
    if len(removed_names) != len(set(removed_names)):
        raise StateError("finish journal contains duplicate removed checkout names")
    if not set(removed_names) <= {checkout.name for checkout in record.checkouts}:
        raise StateError("finish journal names an unknown removed checkout")
    salvage = tuple(
        _as_mapping(item, f"finish journal.salvage[{index}]")
        for index, item in enumerate(
            _as_list(raw.get("salvage", []), "finish journal.salvage")
        )
    )
    if record.slot_type == "validate" and salvage:
        raise StateError("validate finish journal must not contain salvage evidence")
    validate_complete = raw.get("validate_complete", False)
    if not isinstance(validate_complete, bool):
        raise StateError("finish journal.validate_complete must be boolean")
    if validate_complete and record.slot_type != "validate":
        raise StateError("agent finish journal cannot claim validate completion")
    if record.slot_type == "agent" and salvage:
        names = {
            _as_str(item.get("checkout"), "finish journal.salvage checkout")
            for item in salvage
        }
        top_level = {checkout.name for checkout in record.checkouts}
        if not top_level <= names or any(
            not any(name == top or name.startswith(f"{top}/") for top in top_level)
            for name in names
        ):
            raise StateError(
                "finish journal salvage does not cover every checkout or names an "
                "unrelated nested repository"
            )
    current = next((item for item in state.slots if item.slot == record.slot), None)
    archive = _load_archive(config)
    archive_id = _as_str(raw["archive_id"], "finish journal.archive_id")
    already_archived = any(item.get("archive_id") == archive_id for item in archive.records)
    if current is None and already_archived:
        expected_entry = _archive_entry(raw, record)
        matching_entry = next(
            item for item in archive.records if item.get("archive_id") == archive_id
        )
        if not _json_equal(matching_entry, expected_entry):
            raise StateError("durable archive entry differs from the finish journal")
        _assert_physical_slot_removed(config, record, _GitVcs(), raw)
        _atomic_write_json(_archive_path(config), _archive_to_obj(archive))
        _clear_journal(config, raw)
        print(f"recovered finish: cleared completed journal for {record.slot}")
        return
    if current is None:
        raise StateError("finish journal has neither its active record nor a durable archive entry")
    if _record_to_obj(current) != _record_to_obj(record):
        raise StateError("finish journal record does not exactly match ACTIVE")
    _assert_caller_process(coordinator, "coordinator")
    _assert_not_held(config, current)
    age, expired = _heartbeat_diagnosis(current)
    owner_state, detail = _process_state(current.owner)
    live_validate_owner = bool(
        validate_complete
        and owner_state == "live"
        and current.owner == coordinator
    )
    if live_validate_owner:
        _assert_caller_process(coordinator, "validate owner")
    else:
        _assert_registered_liveness(config, current)
    if not expired and not validate_complete:
        raise Refusal(
            f"slot {current.slot} time-to-live is no longer expired after renewal; "
            "preserve it and do not resume deletion"
        )
    if current.owner is None:
        raise Refusal(
            f"slot {current.slot} has no recorded owner process, so owner death is unknown; "
            "preserve it rather than completing deletion"
        )
    if owner_state != "dead" and not live_validate_owner:
        raise Refusal(f"recorded owner is {owner_state}: {detail}")
    journal = dict(raw)
    phase = _as_str(journal["phase"], "finish journal.phase")
    if phase not in ("prepared", "fenced", "removed"):
        raise StateError(f"unknown finish journal phase {phase!r}")
    if phase in ("prepared", "fenced"):
        journal = _finish_remove_paths(
            config,
            record,
            journal,
            _GitVcs(),
            allow_live_validate_owner=live_validate_owner,
        )
    else:
        if set(removed_names) != {checkout.name for checkout in record.checkouts}:
            raise StateError("removed finish phase does not name every checkout")
        _assert_physical_slot_removed(config, record, _GitVcs(), journal)
    _finish_state_update(config, state, record, journal)
    print(f"recovered finish: archived and removed slot={record.slot}")


def _legacy_validate_recovery_inputs(
    config: Config, raw: Mapping[str, object]
) -> tuple[Path, Path, Path, str]:
    required = {
        "schema",
        "kind",
        "machine",
        "slot",
        "phase",
        "checkout",
        "repository",
        "completed_record",
        "completed_record_sha256",
        "head",
        "actor",
        "coordinator_authorized",
    }
    _exact_keys(raw, required, set(), "legacy validate removal journal")
    if _as_int(raw["schema"], "legacy validate removal journal.schema") != SCHEMA:
        raise StateError("unsupported legacy validate removal journal schema")
    if _as_str(raw["kind"], "legacy validate removal journal.kind") != "legacy-validate-remove":
        raise StateError("legacy validate removal journal has the wrong operation kind")
    if _as_str(raw["machine"], "legacy validate removal journal.machine") != config.machine:
        raise StateError("legacy validate removal journal belongs to a different machine")
    slot = _validate_name(
        _as_str(raw["slot"], "legacy validate removal journal.slot"), "slot"
    )
    checkout_relative, checkout = _relative_inside(
        config.root,
        _as_str(raw["checkout"], "legacy validate removal journal.checkout"),
        "legacy validation checkout",
    )
    if (
        Path(checkout_relative).parent != Path("ignored")
        or checkout.name != slot
        or not slot.startswith("validate-fresh-")
    ):
        raise Refusal(
            "legacy validation cleanup accepts only an exact ignored/validate-fresh-* "
            "checkout. state: REFUSED -- no checkout was removed. remedy: pass the "
            "project-relative checkout recorded by the completed validation handle"
        )
    repository_relative, repository = _stored_repository_path(
        config,
        _as_str(raw["repository"], "legacy validate removal journal.repository"),
    )
    del repository_relative
    record_relative, record_path = _relative_inside(
        config.root,
        _as_str(raw["completed_record"], "legacy validate removal journal.completed_record"),
        "completed validation record",
    )
    if (
        Path(record_relative).parent != Path("ignored/validate/runs")
        or record_path.suffix != ".json"
    ):
        raise Refusal(
            "completed validation evidence must be an exact JSON record under "
            "ignored/validate/runs. state: REFUSED -- no checkout was removed. "
            "remedy: pass the durable run-record path used by validate-run"
        )
    expected_digest = _as_str(
        raw["completed_record_sha256"],
        "legacy validate removal journal.completed_record_sha256",
    )
    if not re.fullmatch(r"[0-9a-f]{64}", expected_digest):
        raise StateError("legacy validate removal journal has an invalid record digest")
    try:
        record_contents = record_path.read_bytes()
    except OSError as exc:
        raise Refusal(
            f"cannot read completed validation record {record_path}: {exc}. state: "
            "REFUSED -- no checkout was removed. remedy: restore the durable record "
            "byte-for-byte and rerun wrkslots recover"
        ) from exc
    observed_digest = hashlib.sha256(record_contents).hexdigest()
    if observed_digest != expected_digest:
        raise Refusal(
            f"completed validation record changed after cleanup was prepared: {record_path}. "
            "state: REFUSED -- no checkout was removed. remedy: preserve the checkout and "
            "inspect the record change before retrying"
        )
    try:
        record_value = json.loads(record_contents)
    except json.JSONDecodeError as exc:
        raise Refusal(f"completed validation record is not valid JSON: {record_path}: {exc}") from exc
    record = _as_mapping(record_value, "completed validation record")
    recorded_checkout = record.get("checkout")
    if not isinstance(recorded_checkout, str) or Path(recorded_checkout).resolve() != checkout:
        raise Refusal(
            "completed validation record does not name this exact checkout. state: "
            "REFUSED -- no checkout was removed. remedy: pass the matching run record "
            "or retain the checkout for inspection"
        )
    if record.get("temporary_checkout") is False:
        raise Refusal(
            "completed validation record says this was not a temporary checkout. state: "
            "REFUSED -- no checkout was removed. remedy: retain it; only validation-created "
            "temporary checkouts use this recovery path"
        )
    expected_exits = {"PASSED": 0, "FAILED": 1, "COULD_NOT_RUN": 75}
    final_status = record.get("final_validate_status")
    if (
        record.get("state") != "completed"
        # Narrows `final_status` for the index below, which reads it out of a decoded record and so
        # sees `object`. Placed before the membership test rather than after it because only the
        # isinstance narrows; the runtime answer is unchanged, since a non-string was never a key.
        or not isinstance(final_status, str)
        or final_status not in expected_exits
        or record.get("exit_code") != expected_exits[final_status]
    ):
        raise Refusal(
            "validation record does not contain an evidenced completed result. state: "
            "REFUSED -- no checkout was removed. remedy: let validate-run finish recording "
            "the result, or retain the checkout while the outcome is unknown"
        )
    head = _as_str(raw["head"], "legacy validate removal journal.head")
    if not SHA_RE.fullmatch(head):
        raise StateError("legacy validate removal journal has an invalid checkout HEAD")
    return checkout, repository, record_path, head


def _legacy_validate_journal(
    config: Config,
    args: argparse.Namespace,
    coordinator: ProcessIdentity,
) -> dict[str, object]:
    checkout_relative, checkout = _relative_inside(
        config.root, args.legacy_validate_checkout, "legacy validation checkout"
    )
    record_relative, record_path = _relative_inside(
        config.root, args.completed_record, "completed validation record"
    )
    repository_relative, repository = _repository_path(config, args.repository)
    if not checkout.is_dir() or checkout.is_symlink():
        raise Refusal(
            f"legacy validation checkout is absent or unsafe: {checkout}. state: "
            "REFUSED -- no lifecycle record was changed. remedy: inspect the durable "
            "validation record; if Git already removed the checkout, prune its stale "
            "registration explicitly before retrying"
        )
    _ensure_no_mount_components(config.root, checkout, "legacy validation checkout")
    _assert_no_mountinfo_crossing(checkout, checkout, "legacy validation checkout")
    if (checkout / "HANDOFF.md").exists() or (checkout / "HANDOFF.md").is_symlink():
        raise Refusal(
            f"legacy validation checkout contains HANDOFF.md: {checkout / 'HANDOFF.md'}. "
            "state: REFUSED -- no checkout was removed. remedy: have a participant read "
            "and preserve the handoff before deciding how to recover this directory"
        )
    vcs = _GitVcs()
    head = vcs.verify_existing_worktree(repository, checkout)
    try:
        record_contents = record_path.read_bytes()
    except OSError as exc:
        raise Refusal(f"cannot read completed validation record {record_path}: {exc}") from exc
    journal: dict[str, object] = {
        "schema": SCHEMA,
        "kind": "legacy-validate-remove",
        "machine": config.machine,
        "slot": checkout.name,
        "phase": "prepared",
        "checkout": checkout_relative,
        "repository": repository_relative,
        "completed_record": record_relative,
        "completed_record_sha256": hashlib.sha256(record_contents).hexdigest(),
        "head": head,
        "actor": _identity_to_obj(coordinator),
        "coordinator_authorized": bool(args.coordinator_authorized),
    }
    _legacy_validate_recovery_inputs(config, journal)
    _assert_slot_unused(checkout)
    return journal


def _recover_legacy_validate_remove(
    config: Config,
    path: Path,
    raw: Mapping[str, object],
    coordinator: ProcessIdentity,
) -> None:
    if path != _journal_path(config):
        raise StateError(
            f"legacy validate removal journal filename does not match machine {config.machine}"
        )
    _assert_caller_process(coordinator, "coordinator")
    checkout, repository, record_path, head = _legacy_validate_recovery_inputs(config, raw)
    phase = _as_str(raw["phase"], "legacy validate removal journal.phase")
    if phase not in ("prepared", "removed"):
        raise StateError(f"unknown legacy validate removal phase {phase!r}")
    vcs = _GitVcs()
    listed = vcs.listed_worktrees(repository)
    if checkout.exists() or checkout.is_symlink():
        if phase == "removed":
            raise Refusal(
                f"legacy validation checkout reappeared after removal was recorded: {checkout}. "
                "state: REFUSED -- no further deletion ran. remedy: preserve it and inspect "
                "the repository registration before retrying"
            )
        if checkout.is_symlink() or not checkout.is_dir():
            raise Refusal(f"legacy validation checkout became unsafe: {checkout}")
        _ensure_no_mount_components(config.root, checkout, "legacy validation checkout")
        _assert_no_mountinfo_crossing(checkout, checkout, "legacy validation checkout")
        if (checkout / "HANDOFF.md").exists() or (checkout / "HANDOFF.md").is_symlink():
            raise Refusal(
                f"legacy validation checkout gained HANDOFF.md: {checkout / 'HANDOFF.md'}; "
                "preserve it and do not resume deletion"
            )
        observed_head = vcs.verify_existing_worktree(repository, checkout)
        if observed_head != head:
            raise Refusal(
                f"legacy validation checkout HEAD changed from {head} to {observed_head}; "
                "preserve it and do not resume deletion"
            )
        _assert_slot_unused(checkout)
        vcs.remove_worktree(repository, checkout, force=True)
        _interrupt_for_test("after-legacy-validate-remove")
        listed = vcs.listed_worktrees(repository)
    if checkout.absolute() in listed:
        raise Refusal(
            f"Git still registers removed legacy validation checkout {checkout}. state: "
            "REFUSED -- cleanup is incomplete. remedy: inspect the registration and rerun "
            "wrkslots recover; do not report the checkout removed"
        )
    if checkout.exists() or checkout.is_symlink():
        raise Refusal(
            f"legacy validation checkout still exists after Git removal: {checkout}. state: "
            "REFUSED -- cleanup is incomplete. remedy: preserve the directory and inspect "
            "why Git left it before rerunning recovery"
        )
    journal = dict(raw)
    if phase != "removed":
        journal["phase"] = "removed"
        _write_journal(config, journal)
    _write_event_file(
        config,
        config.machine,
        "legacy-validate-checkout-removed",
        {
            "slot": raw["slot"],
            "checkout": raw["checkout"],
            "repository": raw["repository"],
            "completed_record": raw["completed_record"],
            "completed_record_sha256": raw["completed_record_sha256"],
            "head": head,
            "recovery_actor": _identity_to_obj(coordinator),
        },
    )
    _clear_journal(config, journal)
    print(
        f"recovered completed legacy validation checkout={checkout} "
        f"record={record_path}"
    )


def _cmd_recover(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    coordinator = _capture_caller_process(args.coordinator_pid, "coordinator")
    with _mutation_locks(config, args.wait_lock):
        recovered_partial = _recover_partial_updates(config, args.discard_partial)
        if recovered_partial and not _outstanding_journals(config):
            _ensure_event_log(config)
            _write_event_file(
                config,
                config.machine,
                "partial-updates-recovered",
                {
                    "actor": _identity_to_obj(coordinator),
                    "coordinator_authorized": bool(args.coordinator_authorized),
                },
            )
            states, _archives = _validate_global_state(config)
            _assert_command_registry_storage(config, states, args)
            return 0
        requested_legacy = any(
            value is not None
            for value in (
                args.legacy_validate_checkout,
                args.completed_record,
                args.repository,
            )
        )
        if requested_legacy:
            if not all(
                value is not None
                for value in (
                    args.legacy_validate_checkout,
                    args.completed_record,
                    args.repository,
                )
            ):
                raise Refusal(
                    "legacy validation cleanup requires --legacy-validate-checkout, "
                    "--completed-record, and --repository together. state: REFUSED -- "
                    "no checkout or lifecycle record was changed. remedy: supply all three "
                    "project-relative paths from the completed validate-run record"
                )
            if _outstanding_journals(config):
                raise Refusal(
                    "an interrupted mutation is already recorded. state: REFUSED -- the new "
                    "legacy validation cleanup did not start. remedy: rerun 'wrkslots recover "
                    "--coordinator-pid PID' without legacy cleanup flags to finish the recorded "
                    "operation first"
                )
            states, archives = _validate_global_state(config)
            _assert_command_registry_storage(config, states, args)
            before = _global_rows(states, archives)
            journal = _legacy_validate_journal(config, args, coordinator)
            _write_journal(config, journal)
            _interrupt_for_test("after-legacy-validate-journal")
            path = _journal_path(config)
            raw: Mapping[str, object] = journal
        else:
            path, raw = _load_journal(config)
        _ensure_event_log(config)
        _write_event_file(
            config,
            config.machine,
            "recovery-started",
            {
                "slot": raw.get("slot"),
                "operation": raw.get("kind"),
                "actor": _identity_to_obj(coordinator),
                "coordinator_authorized": bool(args.coordinator_authorized),
            },
        )
        if _as_int(raw.get("schema"), "journal.schema") != SCHEMA:
            raise StateError("unsupported recovery journal schema")
        machine = _as_str(raw.get("machine"), "journal.machine")
        if machine != config.machine:
            raise Refusal(
                f"journal belongs to machine {machine}; rerun with --machine {machine}"
            )
        kind = _as_str(raw.get("kind"), "journal.kind")
        if kind == "finish":
            states, archives = _validate_global_state_for_finish_recovery(config, raw)
        else:
            states, archives = _validate_global_state(config)
        before = _global_rows(states, archives)
        state = _load_active(config)
        if kind == "create":
            _assert_caller_process(coordinator, "coordinator")
            _recover_create(
                config,
                path,
                raw,
                state,
                coordinator,
                retry_running_hook=args.retry_running_hook,
                abort_create=args.abort_create,
            )
        elif kind == "import-existing":
            if args.retry_running_hook or args.abort_create:
                raise Refusal(
                    "--retry-running-hook and --abort-create apply only to create journals"
                )
            _recover_import_existing(config, path, raw, state, coordinator)
        elif kind == "finish":
            if args.retry_running_hook or args.abort_create:
                raise Refusal(
                    "--retry-running-hook and --abort-create apply only to create journals"
                )
            _recover_finish(config, path, raw, state, coordinator)
        elif kind == "legacy-validate-remove":
            if args.retry_running_hook or args.abort_create:
                raise Refusal(
                    "--retry-running-hook and --abort-create apply only to create journals"
                )
            _recover_legacy_validate_remove(config, path, raw, coordinator)
        else:
            raise StateError(f"unknown recovery journal kind {kind!r}")
        after_states, after_archives = _validate_global_state(config)
        after = _global_rows(after_states, after_archives)
        if kind == "legacy-validate-remove":
            if after != before:
                raise StateError(
                    "legacy validation cleanup changed the registered slot state"
                )
        else:
            slot = _as_str(raw.get("slot"), "journal.slot")
            _assert_only_slot_changed(before, after, slot)
    return 0


class _HelpFormatter(argparse.RawDescriptionHelpFormatter):
    def __init__(self, prog: str) -> None:
        super().__init__(prog, max_help_position=30, width=100)


def _add_repo_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--repo",
        action="append",
        metavar="NAME=PATH",
        help=(
            "relative source Git repository path inside the project root, or ../NAME path "
            "components for one direct sibling; absolute paths, other parent traversal, "
            "and symlink components are refused. NAME labels the checkout and must match the "
            "other NAME=VALUE options (repeat once per checkout)"
        ),
    )
    parser.add_argument(
        "--remote",
        action="append",
        metavar="NAME=REMOTE",
        help=(
            "configured Git remote name for one checkout "
            "(default: the remote recorded by init)"
        ),
    )
    parser.add_argument(
        "--remote-url",
        action="append",
        metavar="NAME=URL",
        help=(
            "expected fetch URL for the selected remote; when omitted, record the "
            "configured URL (repeat once per applicable checkout)"
        ),
    )


_QUICKSTART = """\
wrkslots manages durable agent and validation worktree slots.

1. Initialize the project once. The running command is project-owned and is called with the agent
   name. It must return 0 for dead, 1 for alive, or 2 when it cannot determine the answer.

     wrkslots init . --worktrees-dir worktrees/slots --liveness-command tools/agent-liveness.py

2. Create a new linked worktree and branch from an existing source repository. The repository's
   configured origin is used by default; add --remote product=upstream to choose another remote,
   and add --remote-url product=URL when the caller must verify its fetch URL.

     wrkslots create slot01 \\
       --slot-type agent --coordinator-authorized \\
       --agent codex-1 --task task-123 --purpose "fix parser" \\
       --coordinator-pid "$COORDINATOR_PID" --owner-pid "$OWNER_PID" \\
       --repo product=product --branch product=codex/fix-parser

3. The exact recorded owner refreshes its heartbeat while working.

     wrkslots heartbeat slot01 \\
       --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1

4. Before exiting, the owner may record a clean, published handoff. This retains the slot.

     wrkslots finish slot01 \\
       --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1 \\
       --validation "make test: pass"

5. After the registered running command and exact process identity prove the owner dead, and the
   recorded time-to-live expires without renewal, any later coordinator may remove the slot.
   Agent slots publish dirty and unpushed work first; validate slots skip salvage.

     wrkslots remove slot01 \\
       --coordinator-pid "$CURRENT_COORDINATOR_PID" --expected-generation 1

If a create or removal is interrupted, preserve all paths and run:

     wrkslots recover --coordinator-pid "$CURRENT_COORDINATOR_PID"

Use `wrkslots COMMAND --help` for the exact effects and inputs of one command.
"""


def _cmd_quickstart(_args: argparse.Namespace) -> int:
    print(_QUICKSTART)
    return 0


def _load_userguide() -> str:
    return Path(__file__).with_name("USER_GUIDE.md").read_text(encoding="utf-8")


def _build_parser() -> argparse.ArgumentParser:
    description = """Create, track, hand off, recover, and remove Git worktree slots for coding agents.

Read-only: status, doctor, audit, import-existing without --apply, and
clean-caches without --only or --yes. unpushed refreshes remote-tracking refs
but does not mutate registry state.
Mutating: init, create, register, import-existing --apply, adopt,
recover-unbound-owner, heartbeat, hold, unhold, clean-caches deletion, finish,
read-handoff, remove, and recover. Registry mutations take a state lock and append
hash-linked events. ACTIVE and ARCHIVED are compatibility views derived from those events.
Each slot binds an agent identity, coordinator process generation, and one or more linked Git
worktrees to durable machine-sharded state. Removal fails closed unless the time-to-live,
registered running command, process evidence, Git state, salvage evidence, and recovery journal
all agree that deletion is safe.
"""
    epilog = """Lifecycle:
  wrkslots quickstart
  wrkslots init . --worktrees-dir worktrees/slots --liveness-command tools/agent-liveness.py
  wrkslots create slot01 --slot-type agent --coordinator-authorized \\
    --agent codex-1 --task task-123 --purpose "fix parser" \\
    --repo product=product --branch product=codex/fix-parser \\
    --coordinator-pid PID --owner-pid PID
  wrkslots heartbeat slot01 --agent codex-1 --owner-pid PID --expected-generation 1
  wrkslots finish slot01 --agent codex-1 --owner-pid PID --expected-generation 1 \\
    --validation "exact command and result"

finish proves clean index/tracked/untracked/ignored state outside configured regenerable cache
paths, no unfinished Git operation, remote durability, exact landed ancestry, and path identity,
then records the owner-alive handoff while retaining physical storage. Any later participant may
run remove. It proceeds only after the heartbeat TTL expires, the registered liveness command
returns rc 0, the exact owner process generation is dead, and independent process/cgroup/mount
checks prove the slot unused. rc 1, rc 2, and unavailable evidence all refuse.

Any failed precondition leaves every worktree in place. If a process stops during create or
remove, later mutations refuse and 'wrkslots recover' resumes from the append-only history and
its journal compatibility view. Never edit EVENTS.*, ACTIVE.*, ARCHIVED.*, or a journal by hand.

Global options (--project-root, --machine, --wait-lock,
--allow-existing-unregistered-worktrees) precede the command.
Exit status: 0 success/no gate finding, 1 audit gate action, 2 command-line
usage or audit gate unknown, 3 fail-closed refusal.
"""
    parser = argparse.ArgumentParser(
        prog="wrkslots",
        description=description,
        epilog=epilog,
        formatter_class=_HelpFormatter,
    )
    parser.add_argument("--version", action="version", version=f"wrkslots {VERSION}")
    parser.add_argument(
        "--userguide",
        action="store_true",
        help="print the complete installed user guide and exit",
    )
    parser.add_argument(
        "--project-root",
        help=f"project root containing {CONFIG_NAME}; otherwise search upward",
    )
    parser.add_argument(
        "--machine",
        help="override the configured machine shard (or set WRKSLOTS_MACHINE)",
    )
    parser.add_argument(
        "--wait-lock",
        type=float,
        default=0.0,
        metavar="SECONDS",
        help="wait up to this many seconds for a state lock (default: fail immediately)",
    )
    parser.add_argument(
        "--allow-existing-unregistered-worktrees",
        action="store_true",
        help=(
            "permit operations on registered slots while retaining and warning about "
            "pre-existing worktree directories that have no wrkslots record; never treats "
            "those directories as free or authorizes their removal"
        ),
    )
    subparsers = parser.add_subparsers(dest="command", metavar="COMMAND")

    quickstart = subparsers.add_parser(
        "quickstart",
        help="print the normal end-to-end workflow",
        description="Print a concise tutorial for init, create, heartbeat, finish, remove, and recovery.",
        formatter_class=_HelpFormatter,
    )
    quickstart.set_defaults(handler=_cmd_quickstart)

    init = subparsers.add_parser(
        "init",
        help="initialize wrkslots for one project",
        description=(
            "Initialize wrkslots once at a project root. This creates the configuration, empty "
            "machine state, managed worktrees directory, and a project-local command symlink. "
            "It does not create a slot or alter a source repository."
        ),
        epilog=(
            "The running command is project-owned deletion authority. wrkslots invokes it as "
            "PATH AGENT with recorded identity in WRKSLOTS_* environment variables. Exit 0 means "
            "dead, 1 means alive, and 2 means unverifiable; every other result refuses removal."
        ),
        formatter_class=_HelpFormatter,
    )
    init.add_argument(
        "directory",
        nargs="?",
        default=".",
        help="project root to initialize (default: current directory)",
    )
    init.add_argument(
        "--worktrees-dir",
        default="worktrees/slots",
        metavar="PATH",
        help=(
            "managed agent-slot directory relative to the project root; validation slots use "
            "the sibling worktrees/validate directory (default: worktrees/slots)"
        ),
    )
    init.add_argument(
        "--default-remote",
        default="origin",
        metavar="REMOTE",
        help="configured Git remote name used when create omits --remote (default: origin)",
    )
    init.add_argument(
        "--default-landed-ref",
        default="refs/remotes/origin/main",
        metavar="REF",
        help="remote-tracking ref that a finished commit must reach (default: refs/remotes/origin/main)",
    )
    init.add_argument(
        "--heartbeat-ttl-seconds",
        type=int,
        default=3600,
        metavar="SECONDS",
        help=(
            "age after which an unrenewed heartbeat satisfies one reclaim condition; owner and "
            "liveness evidence must independently agree (default: 3600)"
        ),
    )
    init.add_argument(
        "--layout",
        choices=("nested", "flat"),
        default="nested",
        help="nested puts repositories under each slot; flat makes one checkout the slot root",
    )
    init.add_argument(
        "--liveness-command",
        required=True,
        metavar="PATH",
        help=(
            "executable path relative to the project root; called as PATH AGENT during removal "
            "and must return 0 dead, 1 alive, or 2 unverifiable"
        ),
    )
    init.add_argument(
        "--max-active-slots",
        type=int,
        default=None,
        help="refuse create/register beyond this many active slots on this machine "
        "(default: uncapped)",
    )
    init.add_argument(
        "--cache-glob",
        action="append",
        metavar="PATH-GLOB",
        help="checkout-relative regenerable directory glob applied to every repository",
    )
    init.add_argument(
        "--repo-cache-glob",
        action="append",
        metavar="NAME=PATH-GLOB",
        help="regenerable directory glob applied only to repository NAME (repeatable)",
    )
    init.add_argument(
        "--post-provision-hook",
        action="append",
        metavar="SHELL-COMMAND",
        help="ordered shell command run in every new checkout (repeatable)",
    )
    init.add_argument("--disk-advisory-gib", type=int)
    init.add_argument("--disk-provisioning-floor-gib", type=int)
    init.add_argument("--disk-emergency-gib", type=int)
    init.add_argument(
        "--repair",
        action="store_true",
        help="migrate files written by an older build: repair compatible "
        "configuration, exact empty schema-1 machine state, and the previous "
        "repository command symlink; refuse conflicting or populated state",
    )
    init.set_defaults(handler=_cmd_init)

    status = subparsers.add_parser(
        "status",
        help="show active slots and safety diagnoses",
        description=(
            "Read durable state without mutating it. Reports ownership, heartbeat age, process "
            "evidence, Git state, journals, and cross-machine conflicts. A diagnosis never "
            "authorizes deletion."
        ),
        formatter_class=_HelpFormatter,
    )
    status.add_argument(
        "--all-machines",
        action="store_true",
        help="read every ACTIVE machine shard instead of only the selected machine",
    )
    status.add_argument("--slot", metavar="SLOT", help="limit output to this slot name")
    status.add_argument(
        "--format",
        choices=("human", "json"),
        default="human",
        help="output format (default: human)",
    )
    status.set_defaults(handler=_cmd_status)

    doctor = subparsers.add_parser(
        "doctor",
        description=(
            "read-only: list every registry/storage disagreement at once "
            "instead of refusing at the first; authorizes nothing"
        ),
        help="list every registry/storage disagreement at once (read-only)",
    )
    doctor.add_argument("--all-machines", action="store_true", help="read every ACTIVE shard")
    doctor.add_argument("--format", choices=("human", "json"), default="human")
    doctor.set_defaults(handler=_cmd_doctor)

    audit = subparsers.add_parser(
        "audit",
        description="read-only leak audit with per-slot deletion verdicts",
        help="show leak counts and DELETABLE/BLOCKED/HELD verdicts",
    )
    audit.add_argument("--format", choices=("human", "json"), default="human")
    audit.add_argument(
        "--gate",
        action="store_true",
        help=(
            "exit 1 when a DELETABLE slot, interrupted operation, or unregistered "
            "directory needs coordinator attention; exit 2 when an expired slot's "
            "evidence is unavailable; blocked live evidence is retained without a finding"
        ),
    )
    audit.set_defaults(handler=_cmd_audit)

    unpushed = subparsers.add_parser(
        "unpushed",
        description="report remote containment and two-readings evidence for every HEAD",
        help="diagnose unpublished or stale-after-rebase local tips",
    )
    unpushed.add_argument("--slot")
    unpushed.add_argument("--format", choices=("human", "json"), default="human")
    unpushed.set_defaults(handler=_cmd_unpushed)

    create = subparsers.add_parser(
        "create",
        help="create linked worktrees and register a slot",
        description=(
            "Create one new branch and linked Git worktree for each --repo, then durably register "
            "the slot, coordinator, and optional owner. Source repositories and configured "
            "remotes must already exist. create never reclaims or removes another slot."
        ),
        epilog=(
            "Each NAME must appear once in --repo and --branch. --start, --remote, --remote-url, "
            "and later Git evidence use the same NAME. The configured origin is used by default. "
            "When --remote-url is supplied, the selected remote must match it; otherwise its "
            "configured URL is recorded. Omit --owner-pid only when that owner will immediately "
            "run adopt."
        ),
        formatter_class=_HelpFormatter,
    )
    create.add_argument("slot", help="new slot name, such as slot01")
    create.add_argument(
        "--slot-type",
        choices=SLOT_TYPES,
        help="agent preserves authored work before reclaim; validate never salvages",
    )
    create.add_argument(
        "--coordinator-authorized",
        action="store_true",
        help=(
            "confirm that the coordinator assigned this slot; agents should ask the "
            "coordinator instead of passing this flag on their own"
        ),
    )
    create.add_argument("--agent", required=True, metavar="AGENT", help="registered agent name")
    create.add_argument("--task", required=True, metavar="TASK", help="task or work-item identifier")
    create.add_argument("--purpose", required=True, metavar="TEXT", help="short human-readable reason for the slot")
    create.add_argument(
        "--owner-pid",
        type=int,
        metavar="PID",
        help="live owner PID to bind; omit only when the owner will immediately adopt",
    )
    create.add_argument(
        "--coordinator-pid",
        type=int,
        required=True,
        metavar="PID",
        help="live coordinator PID; it must be an ancestor of this command",
    )
    _add_repo_options(create)
    create.add_argument(
        "--branch",
        action="append",
        metavar="NAME=BRANCH",
        help=(
            "new local branch for one checkout (required once per --repo for agent slots; "
            "validate slots generate private branches when omitted)"
        ),
    )
    create.add_argument(
        "--start",
        action="append",
        metavar="NAME=REF",
        help="start ref for one checkout (default: landed ref derived for its configured remote)",
    )
    create.add_argument(
        "--override-disk-floor",
        action="store_true",
        help="allow create below the provisioning floor, but never below the emergency floor",
    )
    create.add_argument("--format", choices=("human", "json"), default="human")
    create.set_defaults(handler=_cmd_create)

    register = subparsers.add_parser(
        "register",
        help="register already-created live worktrees",
        description=(
            "Verify linked worktrees already present under the slot path and register their exact "
            "Git, coordinator, and owner identities. This command does not create or move paths. "
            "Use only while the owner is demonstrably live. The owner may be the invoking "
            "process or another child of the invoking coordinator whose working directory is "
            "inside the slot."
        ),
        formatter_class=_HelpFormatter,
    )
    register.add_argument("slot", help="existing managed slot name")
    register.add_argument(
        "--slot-type", choices=SLOT_TYPES,
        help="type recorded at registration; it controls whether reclaim must salvage",
    )
    register.add_argument(
        "--coordinator-authorized", action="store_true",
        help="confirm that the coordinator authorized this registration",
    )
    register.add_argument("--agent", required=True, metavar="AGENT", help="registered agent name")
    register.add_argument("--task", required=True, metavar="TASK", help="task or work-item identifier")
    register.add_argument("--purpose", required=True, metavar="TEXT", help="short human-readable reason for the slot")
    register.add_argument("--owner-pid", type=int, required=True, metavar="PID", help="live owner PID to bind")
    register.add_argument("--coordinator-pid", type=int, required=True, metavar="PID", help="live coordinator PID to bind")
    register.add_argument(
        "--verified-live",
        action="store_true",
        required=True,
        help="confirm that the named worktrees are currently owned and in use",
    )
    _add_repo_options(register)
    register.set_defaults(handler=_cmd_register)

    import_existing = subparsers.add_parser(
        "import-existing",
        help="verify an existing slot and print the registration; dry-run by default",
        description=(
            "Inspect worktrees already present under a slot path and print the registry row that "
            "would describe them. The default is read-only. A live import requires the same "
            "explicit owner evidence as register: another coordinator child is accepted only "
            "while its working directory is inside the slot. --from-state-file instead reads "
            "one exact version 3 historical row, including its recorded process generation, "
            "and begins a fresh heartbeat time-to-live without treating prior quarantine or "
            "release state as permission to delete. Applied imports are journaled and "
            "recoverable."
        ),
        epilog=(
            "Example dry run:\n"
            "  wrkslots import-existing slot01 --slot-type agent --agent codex-1 --task task-123 "
            "--purpose 'continue task-123' --repo product=../product\n"
            "Apply after verifying the owner is live:\n"
            "  wrkslots import-existing slot01 --slot-type agent --coordinator-authorized "
            "--agent codex-1 --task task-123 "
            "--purpose 'continue task-123' --repo product=../product --apply "
            "--verified-live --owner-pid PID --coordinator-pid PID"
        ),
        formatter_class=_HelpFormatter,
    )
    import_existing.add_argument("slot", help="existing managed slot name")
    import_existing.add_argument(
        "--slot-type", choices=SLOT_TYPES,
        help="type to print or record; it controls whether reclaim must salvage",
    )
    import_existing.add_argument(
        "--coordinator-authorized", action="store_true",
        help="confirm that the coordinator authorized this import",
    )
    import_existing.add_argument("--agent", metavar="AGENT", help="live agent name; historical import reads it from the source row")
    import_existing.add_argument("--task", metavar="TASK", help="live task identifier; historical import reads it from the source row")
    import_existing.add_argument("--purpose", metavar="TEXT", help="live purpose; historical import reads it from the source row")
    import_existing.add_argument("--owner-pid", type=int, metavar="PID", help="live owner PID; required with --apply")
    import_existing.add_argument("--coordinator-pid", type=int, metavar="PID", help="live coordinator PID; required with --apply")
    import_existing.add_argument("--verified-live", action="store_true", help="confirm the existing worktrees are live; required with --apply")
    import_existing.add_argument(
        "--from-state-file",
        metavar="PATH",
        help=(
            "project-relative version 3 worktree-state.json whose exact row supplies "
            "historical owner, task, purpose, and checkout-path evidence"
        ),
    )
    import_existing.add_argument(
        "--source-host-id",
        metavar="ID",
        help=(
            "stable /etc/machine-id of the host that produced --from-state-file; "
            "required because the older owner_sidecar did not record it"
        ),
    )
    import_existing.add_argument("--apply", action="store_true", help="write the verified row instead of only printing it")
    _add_repo_options(import_existing)
    import_existing.set_defaults(handler=_cmd_import_existing)

    adopt = subparsers.add_parser(
        "adopt",
        help="bind an unowned slot to its live owner",
        description=(
            "Bind the exact live process generation that will own a newly created slot. This is "
            "only valid for a slot created without --owner-pid and does not transfer an existing "
            "owner."
        ),
        formatter_class=_HelpFormatter,
    )
    adopt.add_argument("slot", help="active slot awaiting an owner")
    adopt.add_argument("--agent", required=True, metavar="AGENT", help="agent name already recorded on the slot")
    adopt.add_argument("--owner-pid", type=int, required=True, metavar="PID", help="live owner PID to bind")
    adopt.add_argument("--expected-generation", type=int, required=True, metavar="N", help="current slot generation; refuses stale callers")
    adopt.set_defaults(handler=_cmd_adopt)

    recover_unbound = subparsers.add_parser(
        "recover-unbound-owner",
        help="record coordinator recovery evidence without replacing historical ownership",
        description=(
            "Record explicit coordinator evidence for a historical slot that has no usable owner "
            "lease. This does not invent or replace an owner identity, prove owner death, make the "
            "slot reclaimable, or remove it."
        ),
        formatter_class=_HelpFormatter,
    )
    recover_unbound.add_argument("slot", help="active slot with no usable owner lease")
    recover_unbound.add_argument("--coordinator-pid", type=int, required=True, metavar="PID", help="recorded live coordinator PID")
    recover_unbound.add_argument("--expected-generation", type=int, required=True, metavar="N", help="current slot generation")
    recover_unbound.add_argument("--recovery-note", required=True, metavar="TEXT", help="why owner evidence cannot be recovered")
    recover_unbound.add_argument("--validation", action="append", required=True, metavar="TEXT", help="validation evidence (repeatable)")
    recover_unbound.add_argument("--limitation", action="append", metavar="TEXT", help="known limitation to retain (repeatable)")
    recover_unbound.set_defaults(handler=_cmd_recover_unbound_owner)

    heartbeat = subparsers.add_parser(
        "heartbeat",
        help="refresh diagnosis data for the exact owner",
        description=(
            "Refresh the heartbeat only when the caller, agent, PID, and process start identity "
            "still match the recorded owner. Expiry is necessary for reclaim but never sufficient."
        ),
        formatter_class=_HelpFormatter,
    )
    heartbeat.add_argument("slot", help="active slot name")
    heartbeat.add_argument("--agent", required=True, metavar="AGENT", help="recorded agent name")
    heartbeat.add_argument("--owner-pid", type=int, required=True, metavar="PID", help="recorded live owner PID")
    heartbeat.add_argument("--expected-generation", type=int, required=True, metavar="N", help="current slot generation")
    heartbeat.set_defaults(handler=_cmd_heartbeat)

    hold = subparsers.add_parser(
        "hold",
        description="protect one active slot from cache cleanup and removal",
        help="protect a slot from cache cleanup and removal",
    )
    hold.add_argument("slot")
    hold.add_argument("--reason", required=True)
    hold.set_defaults(handler=_cmd_hold)

    unhold = subparsers.add_parser(
        "unhold",
        description="release a slot hold",
        help="release a slot hold",
    )
    unhold.add_argument("slot")
    unhold.set_defaults(handler=_cmd_unhold)

    read_handoff = subparsers.add_parser(
        "read-handoff",
        description=(
            "print an agent slot's HANDOFF.md and append its exact digest to the slot history; "
            "remove refuses while the current file has not been read"
        ),
        help="print HANDOFF.md and record that its exact content was read",
    )
    read_handoff.add_argument("slot")
    read_handoff.add_argument(
        "--coordinator-authorized", action="store_true",
        help="record coordinator authorization as optional read provenance",
    )
    read_handoff.add_argument(
        "--coordinator-pid", type=int, required=True, metavar="PID",
        help="live current coordinator PID recorded with the read event",
    )
    read_handoff.set_defaults(handler=_cmd_read_handoff)

    clean_caches = subparsers.add_parser(
        "clean-caches",
        description=(
            "report or remove configured regenerable cache directories without source/liveness gates"
        ),
        help="report or remove configured regenerable cache directories",
    )
    selection = clean_caches.add_mutually_exclusive_group()
    selection.add_argument("--only", action="append", metavar="SLOT")
    selection.add_argument(
        "--yes",
        action="store_true",
        help="remove configured caches from every unheld active or unregistered slot",
    )
    clean_caches.add_argument("--format", choices=("human", "json"), default="human")
    clean_caches.set_defaults(handler=_cmd_clean_caches)

    finish = subparsers.add_parser(
        "finish",
        help="record a clean, published owner handoff",
        description=(
            "Run while the recorded owner is still alive. Verify every checkout is clean, has no "
            "unfinished Git operation, is published to its recorded remote, and is contained by "
            "the landed ref; then retain the slot and record the handoff. finish never deletes."
        ),
        formatter_class=_HelpFormatter,
    )
    finish.add_argument("slot", help="active slot name")
    finish.add_argument("--agent", required=True, metavar="AGENT", help="recorded agent name")
    finish.add_argument("--owner-pid", type=int, required=True, metavar="PID", help="recorded live owner PID")
    finish.add_argument("--expected-generation", type=int, required=True, metavar="N", help="current slot generation")
    finish.add_argument("--validation", action="append", required=True, metavar="TEXT", help="command and result evidence (repeatable)")
    finish.add_argument("--limitation", action="append", metavar="TEXT", help="known limitation to retain (repeatable)")
    finish.set_defaults(handler=_cmd_finish)

    remove = subparsers.add_parser(
        "remove",
        help="salvage when required and remove a reclaimable slot",
        description=(
            "Physical removal that any later participant may complete from durable state. Requires "
            "the heartbeat TTL to be expired, the registered running command to return dead, the "
            "exact owner process generation to be absent, and independent process, mount, Git, "
            "and path checks to agree. Agent slots are salvaged to their recorded remote first; "
            "validate slots skip salvage. Ambiguity refuses."
        ),
        formatter_class=_HelpFormatter,
    )
    remove.add_argument("slot", help="active slot name whose reclaim conditions are satisfied")
    remove.add_argument(
        "--coordinator-authorized", action="store_true",
        help=(
            "record that this cleanup was coordinator-authorized; optional provenance only, "
            "never a gate on helping an abandoned operation"
        ),
    )
    remove.add_argument(
        "--validate-complete",
        action="store_true",
        help=(
            "remove a completed validate slot immediately when the current caller is its exact "
            "owner, or after owner death without waiting for TTL; process-use, path, and Git "
            "checks remain"
        ),
    )
    remove.add_argument("--coordinator-pid", type=int, required=True, metavar="PID", help="live current coordinator PID recorded with the operation")
    remove.add_argument("--expected-generation", type=int, required=True, metavar="N", help="current slot generation")
    remove.set_defaults(handler=_cmd_remove)

    recover = subparsers.add_parser(
        "recover",
        help="resume an interrupted mutation from its journal",
        description=(
            "Validate and resume one durable create or removal from append-only history and its "
            "journal compatibility view. Any later participant may help; mismatched registry, "
            "path, process, or Git evidence refuses. Do not edit the history or journal by hand."
        ),
        formatter_class=_HelpFormatter,
    )
    recover.add_argument("--coordinator-pid", type=int, required=True, metavar="PID", help="live current coordinator PID recorded with recovery")
    recover.add_argument(
        "--coordinator-authorized", action="store_true",
        help="record coordinator authorization as optional recovery provenance",
    )
    recover.add_argument(
        "--discard-partial",
        action="store_true",
        help="discard an incomplete temp write only when its prior durable file is valid",
    )
    recover.add_argument(
        "--legacy-validate-checkout",
        metavar="PATH",
        help=(
            "project-relative ignored/validate-fresh-* worktree from the historical layout; "
            "requires --completed-record and --repository"
        ),
    )
    recover.add_argument(
        "--completed-record",
        metavar="PATH",
        help=(
            "project-relative ignored/validate/runs/*.json record proving the historical "
            "validation completed"
        ),
    )
    recover.add_argument(
        "--repository",
        metavar="PATH",
        help="project-relative source repository that owns the historical Git worktree",
    )
    create_recovery = recover.add_mutually_exclusive_group()
    create_recovery.add_argument(
        "--retry-running-hook",
        action="store_true",
        help="after inspection, rerun a hook whose journal status is ambiguously 'running'",
    )
    create_recovery.add_argument(
        "--abort-create",
        action="store_true",
        help="after inspection, remove an incomplete create's unchanged worktrees and branches",
    )
    recover.set_defaults(handler=_cmd_recover)
    return parser


def _refusal_remedy(args: argparse.Namespace, error: Refusal) -> str:
    command = getattr(args, "command", None)
    instruction = error.remedy
    message = str(error)
    if instruction is None:
        inline = re.search(r"(?:^|[.] )remedy: (.+)$", message, flags=re.IGNORECASE)
        if inline is not None:
            instruction = inline.group(1)
    if instruction is not None:
        if "wrkslots" in instruction or not isinstance(command, str) or not command:
            return instruction
        return (
            f"{instruction}; then rerun 'wrkslots {command}' with the required arguments"
        )
    if isinstance(command, str) and command:
        return (
            f"run 'wrkslots {command} --help', correct the named condition, then rerun "
            f"'wrkslots {command}' with the required arguments"
        )
    return "run 'wrkslots --help', correct the named condition, then rerun wrkslots"


def main(argv: Sequence[str] | None = None) -> int:
    """Run the command-line interface and return its process exit status."""

    parser = _build_parser()
    values = list(sys.argv[1:] if argv is None else argv)
    if not values:
        parser.print_help()
        return 0
    args = parser.parse_args(values)
    if args.userguide:
        guide = _load_userguide()
        print(guide, end="" if guide.endswith("\n") else "\n")
        return 0
    if args.wait_lock < 0 or not args.wait_lock < float("inf"):
        parser.error("--wait-lock must be a finite non-negative number")
    if args.command == "init":
        if args.heartbeat_ttl_seconds <= 0:
            parser.error("--heartbeat-ttl-seconds must be positive")
        if args.project_root is not None:
            parser.error("init uses its DIRECTORY argument, not --project-root")
        if args.max_active_slots is not None and args.max_active_slots < 0:
            parser.error("--max-active-slots must be non-negative")
        for option, value in (
            ("--disk-advisory-gib", args.disk_advisory_gib),
            ("--disk-provisioning-floor-gib", args.disk_provisioning_floor_gib),
            ("--disk-emergency-gib", args.disk_emergency_gib),
        ):
            if value is not None and value <= 0:
                parser.error(f"{option} must be positive")
    handler = getattr(args, "handler", None)
    if handler is None:
        parser.print_help()
        return 0
    try:
        result = handler(args)
    except Refusal as exc:
        print(f"REFUSED: {exc}", file=sys.stderr)
        print(f"REMEDY: {_refusal_remedy(args, exc)}", file=sys.stderr)
        return 3
    return int(result)


if __name__ == "__main__":
    raise SystemExit(main())
