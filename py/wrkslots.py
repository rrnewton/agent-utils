#!/usr/bin/env python3
"""Safely manage one active agent per Git worktree slot."""

from __future__ import annotations

import argparse
import contextlib
import dataclasses
import datetime as dt
import errno
import fcntl
import hashlib
import json
import os
import re
import socket
import stat
import subprocess
import sys
import time
import uuid
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path


VERSION = "0.3.0"
SCHEMA = 2
CONFIG_NAME = ".wrkslots.yml"
NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
REMOTE_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")


class Refusal(RuntimeError):
    """An expected fail-closed refusal with an actionable message."""


class StateError(Refusal):
    """A corrupt, partial, or incompatible state refusal."""


@dataclasses.dataclass(frozen=True)
class Config:
    root: Path
    config_path: Path
    worktrees: Path
    machine: str
    default_remote: str
    default_landed_ref: str
    heartbeat_ttl_seconds: int
    liveness_command: Path


@dataclasses.dataclass(frozen=True)
class ProcessIdentity:
    pid: int
    start_ticks: int
    boot_id: str
    host_id: str
    cgroup_path: str


@dataclasses.dataclass(frozen=True)
class Checkout:
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
    recorded_at: str
    validation: tuple[str, ...]
    limitations: tuple[str, ...]
    continuation: str


@dataclasses.dataclass(frozen=True)
class ActiveRecord:
    slot: str
    agent: str
    task: str
    purpose: str
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


@dataclasses.dataclass(frozen=True)
class ActiveState:
    machine: str
    revision: int
    slots: tuple[ActiveRecord, ...]


@dataclasses.dataclass(frozen=True)
class ArchiveState:
    machine: str
    revision: int
    records: tuple[dict[str, object], ...]


@dataclasses.dataclass(frozen=True)
class PlannedCheckout:
    name: str
    destination: str
    repository: str
    branch: str
    start_point: str
    remote: str
    remote_url_sha256: str
    landed_ref: str


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
    value: Mapping[str, object], required: set[str], optional: set[str], label: str
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


def _path_is_within(candidate: Path, parent: Path) -> bool:
    try:
        candidate.relative_to(parent)
        return True
    except ValueError:
        return False


def _config_payload(
    worktrees_dir: str,
    machine: str,
    default_remote: str,
    default_landed_ref: str,
    heartbeat_ttl_seconds: int,
    liveness_command: str,
) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "worktrees_dir": worktrees_dir,
        "machine": machine,
        "default_remote": default_remote,
        "default_landed_ref": default_landed_ref,
        "heartbeat_ttl_seconds": heartbeat_ttl_seconds,
        "liveness_command": liveness_command,
    }


def _discover_root(explicit: str | None) -> Path:
    if explicit is not None:
        candidate = Path(explicit).absolute()
        if candidate.is_symlink() or not candidate.is_dir():
            raise Refusal(f"project root must be an existing real directory: {candidate}")
        return candidate.resolve(strict=True)
    current = Path.cwd().resolve(strict=True)
    for candidate in (current, *current.parents):
        config_path = candidate / CONFIG_NAME
        if config_path.exists() or config_path.is_symlink():
            return candidate
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
    _exact_keys(raw, required, set(), "configuration")
    if _as_int(raw["schema"], "configuration.schema") != SCHEMA:
        raise StateError(f"unsupported configuration schema in {path}")
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
    return Config(
        root=root,
        config_path=path,
        worktrees=worktrees,
        machine=machine,
        default_remote=remote,
        default_landed_ref=landed_ref,
        heartbeat_ttl_seconds=_as_int(
            raw["heartbeat_ttl_seconds"],
            "configuration.heartbeat_ttl_seconds",
            minimum=1,
        ),
        liveness_command=liveness_command,
    )


def _active_path(config: Config, machine: str | None = None) -> Path:
    selected = machine or config.machine
    _validate_name(selected, "machine")
    return config.worktrees / f"ACTIVE.{selected}.json"


def _archive_path(config: Config, machine: str | None = None) -> Path:
    selected = machine or config.machine
    _validate_name(selected, "machine")
    return config.worktrees / f"ARCHIVED.{selected}.json"


def _journal_path(config: Config, machine: str | None = None) -> Path:
    selected = machine or config.machine
    _validate_name(selected, "machine")
    return config.worktrees / f"ACTIVE.{selected}.journal"


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
    lock_path = path.with_name(f"{path.name}.lock")
    try:
        fd = os.open(
            lock_path,
            os.O_RDWR | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW,
            0o644,
        )
    except OSError as exc:
        raise Refusal(f"cannot open configuration lock {lock_path}: {exc}") from exc
    try:
        mode = os.fstat(fd).st_mode
    except OSError as exc:
        os.close(fd)
        raise Refusal(f"cannot inspect configuration lock {lock_path}: {exc}") from exc
    if not stat.S_ISREG(mode):
        os.close(fd)
        raise Refusal(f"configuration lock is not a regular file: {lock_path}")
    deadline = time.monotonic() + wait_seconds
    try:
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError as exc:
                if time.monotonic() >= deadline:
                    raise Refusal(
                        f"configuration lock is busy for {lock_path}; retry after init exits"
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
    global_subject = config.worktrees / "ACTIVE"
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


def _checkout_to_obj(checkout: Checkout) -> dict[str, object]:
    return dataclasses.asdict(checkout)


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
    return {
        "slot": record.slot,
        "agent": record.agent,
        "task": record.task,
        "purpose": record.purpose,
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


def _record_from_obj(value: object, label: str) -> ActiveRecord:
    raw = _as_mapping(value, label)
    fields = {
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
    _exact_keys(raw, fields, set(), label)
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
    )
    _validate_name(record.slot, "slot")
    _validate_name(record.agent, "agent")
    _validate_name(record.machine, "machine")
    if not record.task or not record.purpose:
        raise StateError(f"{label} has an empty task or purpose")
    _parse_timestamp(record.created_at, f"{label}.created_at")
    _parse_timestamp(record.heartbeat_at, f"{label}.heartbeat_at")
    if not record.checkouts:
        raise StateError(f"{label} has no checkouts")
    if record.coordinator_recovery_note is not None and record.owner is not None:
        raise StateError(f"{label} has coordinator recovery evidence for a bound owner")
    names = [item.name for item in record.checkouts]
    if len(names) != len(set(names)):
        raise StateError(f"{label} has duplicate checkout names")
    return record


def _load_active(config: Config, machine: str | None = None) -> ActiveState:
    selected = machine or config.machine
    path = _active_path(config, selected)
    raw = _as_mapping(_read_json(path, "active state"), "active state")
    _exact_keys(raw, {"schema", "machine", "revision", "slots"}, set(), "active state")
    if _as_int(raw["schema"], "active state.schema") != SCHEMA:
        raise StateError(f"unsupported active state schema in {path}")
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
    agents = [item.agent for item in slots]
    if len(slot_names) != len(set(slot_names)):
        raise StateError(f"duplicate slot in {path}")
    if len(agents) != len(set(agents)):
        raise StateError(f"one agent owns more than one active slot in {path}")
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


def _active_to_obj(state: ActiveState) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "machine": state.machine,
        "revision": state.revision,
        "slots": [_record_to_obj(item) for item in sorted(state.slots, key=lambda x: x.slot)],
    }


def _load_archive(config: Config, machine: str | None = None) -> ArchiveState:
    selected = machine or config.machine
    path = _archive_path(config, selected)
    raw = _as_mapping(_read_json(path, "archive state"), "archive state")
    _exact_keys(raw, {"schema", "machine", "revision", "records"}, set(), "archive state")
    if _as_int(raw["schema"], "archive state.schema") != SCHEMA:
        raise StateError(f"unsupported archive state schema in {path}")
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
        _exact_keys(mapping, fields, set(), label)
        archive_id = _as_str(mapping["archive_id"], f"{label}.archive_id")
        if archive_id in archive_ids:
            raise StateError(f"duplicate archive_id in {path}: {archive_id}")
        archive_ids.add(archive_id)
        slot = _validate_name(_as_str(mapping["slot"], f"{label}.slot"), "archived slot")
        if slot in archived_slots:
            raise StateError(f"duplicate archived slot in {path}: {slot}")
        archived_slots.add(slot)
        agent = _validate_name(
            _as_str(mapping["agent"], f"{label}.agent"), "archived agent"
        )
        if not _as_str(mapping["task"], f"{label}.task") or not _as_str(
            mapping["purpose"], f"{label}.purpose"
        ):
            raise StateError(f"{label} has an empty task or purpose")
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
        if not checkouts or len({checkout.name for checkout in checkouts}) != len(checkouts):
            raise StateError(f"{label} has no checkouts or duplicate checkout names")
        for checkout in checkouts:
            expected, _destination = _checkout_path(config, slot, checkout.name)
            if checkout.path != expected:
                raise StateError(
                    f"{label} checkout {checkout.name} path escaped its archived slot"
                )
            if (
                checkout.remote != config.default_remote
                or checkout.landed_ref != config.default_landed_ref
            ):
                raise StateError(
                    f"{label} checkout {checkout.name} differs from configured authority"
                )
        records.append(dict(mapping))
    return ArchiveState(
        machine=actual_machine,
        revision=_as_int(raw["revision"], "archive state.revision"),
        records=tuple(records),
    )


def _ensure_state_shard(config: Config) -> None:
    active = _active_path(config)
    archive = _archive_path(config)
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
        return
    if active_exists:
        _load_active(config)
        _load_archive(config)
        return
    _atomic_write_json(active, _active_to_obj(ActiveState(config.machine, 0, ())))
    _atomic_write_json(archive, _archive_to_obj(ArchiveState(config.machine, 0, ())))


def _archive_to_obj(state: ArchiveState) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "machine": state.machine,
        "revision": state.revision,
        "records": list(state.records),
    }


def _state_files(config: Config) -> list[tuple[str, Path]]:
    result: list[tuple[str, Path]] = []
    pattern = re.compile(r"^ACTIVE\.(?P<machine>[A-Za-z0-9][A-Za-z0-9._-]{0,63})\.json$")
    for path in sorted(config.worktrees.glob("ACTIVE.*.json")):
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
    for path in sorted(config.worktrees.glob("ARCHIVED.*.json")):
        match = pattern.fullmatch(path.name)
        if match is None:
            raise StateError(f"unexpected archive-state filename: {path}")
        result.append((match.group("machine"), path))
    return result


def _load_all_active(config: Config) -> list[ActiveState]:
    state_files = _state_files(config)
    archive_files = _archive_files(config)
    state_machines = {machine for machine, _path in state_files}
    archive_machines = {machine for machine, _path in archive_files}
    if state_machines != archive_machines:
        raise StateError(
            "ACTIVE and ARCHIVED machine shards differ: "
            f"active={sorted(state_machines)} archived={sorted(archive_machines)}"
        )
    states = [_load_active(config, machine) for machine, _path in state_files]
    global_slots: dict[str, str] = {}
    global_agents: dict[str, str] = {}
    for state in states:
        for record in state.slots:
            previous_slot = global_slots.setdefault(record.slot, state.machine)
            if previous_slot != state.machine:
                raise StateError(
                    f"slot {record.slot!r} is active in both {previous_slot} and {state.machine}"
                )
            previous_agent = global_agents.setdefault(record.agent, record.slot)
            if previous_agent != record.slot:
                raise StateError(
                    f"agent {record.agent!r} owns both {previous_agent} and {record.slot}"
                )
    return states


def _load_all_archives(config: Config) -> list[ArchiveState]:
    archives = [
        _load_archive(config, machine) for machine, _path in _archive_files(config)
    ]
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
) -> None:
    records = [record for state in states for record in state.slots]
    expected_slots = {record.slot for record in records}
    for record in records:
        _assert_slot_contents(config, record)
        vcs = GitVcs()
        for checkout in record.checkouts:
            path = _stored_path(config, checkout.path, "checkout path")
            _relative, repository = _repository_path(config, checkout.repository)
            vcs.verify_existing_worktree(repository, path)
    actual_slots = {
        entry.name
        for entry in config.worktrees.iterdir()
        if entry.is_dir() and not entry.is_symlink()
    }
    unexpected = sorted(actual_slots - expected_slots)
    if allowed_unregistered_slot is not None:
        unexpected = [slot for slot in unexpected if slot != allowed_unregistered_slot]
    if unexpected:
        raise StateError(
            f"managed worktrees directory has a directory without an active row: {unexpected[0]}"
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
    if any(item.agent == record.agent for item in state.slots):
        raise Refusal(f"agent {record.agent!r} already owns an active slot")
    return ActiveState(state.machine, state.revision + 1, (*state.slots, record))


class GitVcs:
    """The small Git boundary used by slot operations."""

    @staticmethod
    def _run(
        repository: Path,
        args: Sequence[str],
        *,
        check: bool = True,
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

    def status(self, checkout: Path) -> str:
        result = self._run(
            checkout,
            [
                "status",
                "--porcelain=v2",
                "--untracked-files=all",
                "--ignored=matching",
            ],
        )
        return result.stdout

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

    def remove_worktree(self, repository: Path, checkout: Path) -> None:
        self._run(repository, ["worktree", "remove", "--", str(checkout)])

    def repair_worktree(self, repository: Path, checkout: Path) -> None:
        self._run(repository, ["worktree", "repair", str(checkout)])
        if checkout.absolute() not in self.listed_worktrees(repository):
            raise Refusal(f"Git did not repair worktree registration for {checkout}")


def _repository_path(config: Config, raw: str) -> tuple[str, Path]:
    candidate = Path(raw)
    if candidate.is_absolute() or ".." in candidate.parts:
        raise Refusal(f"repository path must be relative to {config.root}: {raw!r}")
    normalized = Path(os.path.normpath(str(candidate)))
    if not normalized.parts:
        normalized = Path(".")
    absolute = config.root / normalized
    _ensure_no_symlink_components(config.root, absolute, "repository")
    if not absolute.is_dir() or absolute.is_symlink():
        raise Refusal(f"repository does not exist or is unsafe: {absolute}")
    if _path_is_within(absolute, config.worktrees):
        raise Refusal(
            f"source repository must be outside the managed worktrees directory: {absolute}"
        )
    relative = "." if normalized == Path(".") else normalized.as_posix()
    return relative, absolute


def _checkout_path(config: Config, slot: str, name: str) -> tuple[str, Path]:
    _validate_name(slot, "slot")
    _validate_name(name, "checkout name")
    slot_path = config.worktrees / slot
    destination = slot_path / name
    _ensure_no_symlink_components(config.root, destination, "checkout")
    relative = destination.relative_to(config.root).as_posix()
    return relative, destination


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


def _assert_registered_liveness(config: Config, record: ActiveRecord) -> None:
    env = {
        "LC_ALL": "C",
        "PATH": "/usr/bin:/bin",
        "WRKSLOTS_PROJECT_ROOT": str(config.root),
        "WRKSLOTS_SLOT": record.slot,
        "WRKSLOTS_AGENT": record.agent,
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
        raise Refusal(f"registered liveness command could not run: {exc}") from exc
    detail = (completed.stderr or completed.stdout).strip().splitlines()
    first = detail[0] if detail else "no detail"
    if completed.returncode == 0:
        return
    if completed.returncode == 1:
        raise Refusal(f"registered liveness authority reports owner alive: {first}")
    if completed.returncode == 2:
        raise Refusal(f"registered liveness authority is unverifiable: {first}")
    raise Refusal(
        f"registered liveness command returned unexpected rc {completed.returncode}: {first}"
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
        raise Refusal(f"process use is indeterminate because {path} is unreadable: {exc}") from exc
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


def _assert_slot_unused(slot_path: Path, record: ActiveRecord | None = None) -> None:
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
    if lsof is not None:
        try:
            completed = subprocess.run(
                [str(lsof), "-nP", "-Fpcfn", "+D", str(slot_path)],
                text=True,
                capture_output=True,
                check=False,
            )
        except OSError as exc:
            raise Refusal(f"process use is indeterminate because lsof failed: {exc}") from exc
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
    inspected_mount_namespaces: set[str] = set()
    for pid_dir in sorted(pid_dirs, key=lambda item: int(item.name)):
        pid = int(pid_dir.name)
        if pid == current_pid:
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
        if direct_use_proven_absent:
            mount_namespace = _mount_namespace(pid_dir)
            if mount_namespace is None or mount_namespace in inspected_mount_namespaces:
                uses = []
            else:
                uses = _process_mounts_slot(pid_dir, slot_path)
                inspected_mount_namespaces.add(mount_namespace)
        else:
            uses = _process_uses_slot(pid_dir, slot_path)
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
    expected_slot = config.worktrees / record.slot
    _ensure_no_symlink_components(config.root, expected_slot, "slot")
    for checkout in record.checkouts:
        if (
            checkout.remote != config.default_remote
            or checkout.landed_ref != config.default_landed_ref
        ):
            raise StateError(
                f"slot {record.slot} checkout {checkout.name} differs from configured authority"
            )
        expected_relative, expected_absolute = _checkout_path(config, record.slot, checkout.name)
        if checkout.path != expected_relative:
            raise StateError(
                f"slot {record.slot} checkout {checkout.name} path does not match its slot: "
                f"{checkout.path!r} != {expected_relative!r}"
            )
        actual = _stored_path(config, checkout.path, "checkout path")
        if actual != expected_absolute:
            raise StateError(f"slot {record.slot} checkout path identity changed")
        _repository_path(config, checkout.repository)


def _assert_checkout_identity_unchanged(
    config: Config, checkout: Checkout, vcs: GitVcs
) -> None:
    path = _stored_path(config, checkout.path, "checkout path")
    _relative, repository = _repository_path(config, checkout.repository)
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


def _assert_checkout_safe(config: Config, checkout: Checkout, vcs: GitVcs) -> Checkout:
    path = _stored_path(config, checkout.path, "checkout path")
    _relative, repository = _repository_path(config, checkout.repository)
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
    status = vcs.status(path)
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
    vcs.fetch_remote(path, checkout.remote, checkout.landed_ref)
    if vcs.remote_url_sha256(path, checkout.remote) != checkout.remote_url_sha256:
        raise Refusal(
            f"checkout {checkout.name} remote {checkout.remote} URL changed during fetch; "
            "preserve the checkout for inspection"
        )
    remote_refs = vcs.remote_refs_containing(path, checkout.remote, head)
    if not remote_refs:
        raise Refusal(
            f"checkout {checkout.name} HEAD {head} is not reachable from any "
            f"refs/remotes/{checkout.remote}/* ref; fetch or publish it before finishing"
        )
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


def _assert_slot_contents(config: Config, record: ActiveRecord) -> Path:
    slot_path = config.worktrees / record.slot
    if not slot_path.is_dir() or slot_path.is_symlink():
        raise Refusal(f"slot directory is missing or unsafe: {slot_path}")
    expected = {checkout.name for checkout in record.checkouts}
    try:
        actual = {entry.name for entry in slot_path.iterdir()}
    except OSError as exc:
        raise Refusal(f"cannot inspect slot directory {slot_path}: {exc}") from exc
    extras = sorted(actual - expected)
    missing = sorted(expected - actual)
    if extras or missing:
        details: list[str] = []
        if extras:
            details.append(f"unexpected entries {', '.join(extras)}")
        if missing:
            details.append(f"missing checkouts {', '.join(missing)}")
        raise Refusal(f"slot directory does not match its record: {'; '.join(details)}")
    return slot_path


def _handoff_preconditions(
    config: Config, record: ActiveRecord, vcs: GitVcs
) -> tuple[Path, tuple[Checkout, ...]]:
    _assert_record_paths(config, record)
    slot_path = _assert_slot_contents(config, record)
    final_checkouts = tuple(_assert_checkout_safe(config, item, vcs) for item in record.checkouts)
    return slot_path, final_checkouts


def _remove_preconditions(
    config: Config, record: ActiveRecord, vcs: GitVcs
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


def _refuse_partial_state(config: Config) -> None:
    leftovers = sorted(config.worktrees.glob("ACTIVE.*.json.tmp.*"))
    leftovers += sorted(config.worktrees.glob("ARCHIVED.*.json.tmp.*"))
    leftovers += sorted(config.worktrees.glob("ACTIVE.*.journal.tmp.*"))
    if leftovers:
        raise StateError(
            f"partial atomic update found: {leftovers[0]}; preserve it and use 'wrkslots recover'"
        )


def _outstanding_journals(config: Config) -> list[Path]:
    return sorted(config.worktrees.glob("ACTIVE.*.journal"))


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
    raw = _as_mapping(_read_json(path, "recovery journal"), "recovery journal")
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
    payload = _config_payload(
        worktrees_relative,
        machine,
        remote,
        landed_ref,
        args.heartbeat_ttl_seconds,
        liveness_relative,
    )
    config_path = root / CONFIG_NAME
    with _locked_config(config_path, args.wait_lock):
        _recover_config_write(config_path, payload)
        if config_path.exists() or config_path.is_symlink():
            existing = _as_mapping(_read_json(config_path, "configuration"), "configuration")
            if dict(existing) != payload:
                raise Refusal(
                    f"{config_path} already has different configuration; "
                    "do not move live state by rerunning init"
                )
        else:
            _atomic_write_json(config_path, payload)
    if worktrees.exists() and (not worktrees.is_dir() or worktrees.is_symlink()):
        raise Refusal(f"worktrees directory is not a real directory: {worktrees}")
    worktrees.mkdir(parents=True, exist_ok=True)
    config = Config(
        root=root,
        config_path=config_path,
        worktrees=worktrees,
        machine=machine,
        default_remote=remote,
        default_landed_ref=landed_ref,
        heartbeat_ttl_seconds=args.heartbeat_ttl_seconds,
        liveness_command=liveness_command,
    )
    with _mutation_locks(config, args.wait_lock):
        _recover_partial_updates(config, True)
        _refuse_partial_state(config)
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
        link = worktrees / "wrkslots"
        executable = Path(__file__).resolve()
        relative_target = Path(os.path.relpath(executable, start=worktrees))
        if link.exists() or link.is_symlink():
            if not link.is_symlink():
                raise Refusal(f"control path exists and is not a symlink: {link}")
            if Path(os.readlink(link)) != relative_target:
                raise Refusal(
                    f"control symlink points elsewhere: {link} -> {os.readlink(link)}"
                )
        else:
            link.symlink_to(relative_target)
            _fsync_directory(worktrees)
    print(f"initialized wrkslots in {root}")
    print(f"machine={machine} worktrees={worktrees_relative}")
    print(f"command={link} -> {relative_target}")
    return 0


def _create_plan(config: Config, args: argparse.Namespace, vcs: GitVcs) -> tuple[PlannedCheckout, ...]:
    repositories = _assignments(args.repo, "repository")
    remote_urls = _assignments(args.remote_url, "trusted remote URL")
    branches = _assignments(args.branch, "branch")
    starts = _assignments(args.start, "start point")
    if not repositories:
        raise Refusal("create requires at least one --repo NAME=PATH")
    if set(branches) != set(repositories):
        missing = sorted(set(repositories) - set(branches))
        extra = sorted(set(branches) - set(repositories))
        raise Refusal(
            "branches must exactly match repositories"
            + (f"; missing {', '.join(missing)}" if missing else "")
            + (f"; unknown {', '.join(extra)}" if extra else "")
        )
    if set(remote_urls) != set(repositories):
        missing = sorted(set(repositories) - set(remote_urls))
        extra = sorted(set(remote_urls) - set(repositories))
        raise Refusal(
            "trusted remote URLs must exactly match repositories"
            + (f"; missing {', '.join(missing)}" if missing else "")
            + (f"; unknown {', '.join(extra)}" if extra else "")
        )
    for mapping, label in ((starts, "start point"),):
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
        remote = config.default_remote
        landed_ref = config.default_landed_ref
        if not landed_ref.startswith(f"refs/remotes/{remote}/"):
            raise Refusal(
                f"landed ref for {name} must be under refs/remotes/{remote}/"
            )
        remote_url_sha256 = vcs.assert_remote_url(
            repository, remote, remote_urls[name]
        )
        vcs.fetch_remote(repository, remote, landed_ref)
        if (
            vcs.assert_remote_url(repository, remote, remote_urls[name])
            != remote_url_sha256
        ):
            raise Refusal(f"remote {remote!r} URL changed during fetch")
        start = vcs.verify_ref(
            repository, starts.get(name, landed_ref), "start point"
        )
        destination_relative, destination = _checkout_path(config, args.slot, name)
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


def _assert_agent_and_slot_free(config: Config, slot: str, agent: str) -> None:
    states = _load_all_active(config)
    for state in states:
        for record in state.slots:
            if record.slot == slot:
                raise Refusal(f"slot {slot!r} is already active on {state.machine}")
            if record.agent == agent:
                raise Refusal(
                    f"agent {agent!r} already owns slot {record.slot!r} on {state.machine}"
                )
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
) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "kind": "create",
        "machine": config.machine,
        "slot": args.slot,
        "agent": args.agent,
        "task": args.task,
        "purpose": args.purpose,
        "created_at": created_at,
        "heartbeat_at": heartbeat_at,
        "heartbeat_ttl_seconds": config.heartbeat_ttl_seconds,
        "owner": _identity_to_obj(owner),
        "coordinator_lease": _identity_to_obj(coordinator_lease),
        "planned": [_planned_to_obj(item) for item in plan],
        "created": [_checkout_to_obj(item) for item in created],
    }


def _cmd_create(args: argparse.Namespace) -> int:
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
    vcs = GitVcs()
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_registry_storage_consistent(config, states)
        before = _global_rows(states, archives)
        _ensure_state_shard(config)
        state = _load_active(config)
        _assert_agent_and_slot_free(config, args.slot, args.agent)
        slot_path = config.worktrees / args.slot
        if slot_path.exists() or slot_path.is_symlink():
            raise Refusal(f"slot path already exists: {slot_path}")
        plan = _create_plan(config, args, vcs)
        created_at = _utc_now()
        heartbeat_at = created_at
        journal_path = _journal_path(config)
        created: list[Checkout] = []
        _atomic_write_json(
            journal_path,
            _create_journal_payload(
                config,
                args,
                owner,
                coordinator_lease,
                plan,
                created,
                created_at=created_at,
                heartbeat_at=heartbeat_at,
            ),
        )
        slot_path.mkdir(mode=0o755)
        _fsync_directory(config.worktrees)
        for item in plan:
            _relative, repository = _repository_path(config, item.repository)
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
            _atomic_write_json(
                journal_path,
                _create_journal_payload(
                    config,
                    args,
                    owner,
                    coordinator_lease,
                    plan,
                    created,
                    created_at=created_at,
                    heartbeat_at=heartbeat_at,
                ),
            )
            _interrupt_for_test("after-create-worktree")
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
        _atomic_write_json(
            journal_path,
            _create_journal_payload(
                config,
                args,
                owner,
                coordinator_lease,
                plan,
                created,
                created_at=created_at,
                heartbeat_at=heartbeat_at,
            ),
        )
        record = ActiveRecord(
            slot=args.slot,
            agent=args.agent,
            task=args.task,
            purpose=args.purpose,
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
        _atomic_write_json(_active_path(config), _active_to_obj(state))
        _interrupt_for_test("after-active-write")
        _remove_control_file(journal_path)
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    print(
        f"created slot={record.slot} agent={record.agent} generation={record.generation} "
        f"checkouts={len(record.checkouts)}"
    )
    if owner is None:
        print("owner_process=unbound; run 'wrkslots adopt' before heartbeat or finish")
    return 0


def _existing_checkouts(
    config: Config, args: argparse.Namespace, vcs: GitVcs
) -> tuple[Checkout, ...]:
    repositories = _assignments(args.repo, "repository")
    remote_urls = _assignments(args.remote_url, "trusted remote URL")
    if not repositories:
        raise Refusal("at least one --repo NAME=PATH is required")
    if set(remote_urls) != set(repositories):
        raise Refusal("trusted remote URLs must exactly match repositories")
    checkouts: list[Checkout] = []
    for name, raw_repository in repositories.items():
        repository_relative, repository = _repository_path(config, raw_repository)
        destination_relative, destination = _checkout_path(config, args.slot, name)
        if not destination.is_dir() or destination.is_symlink():
            raise Refusal(f"existing checkout is missing or unsafe: {destination}")
        head = vcs.verify_existing_worktree(repository, destination)
        branch = vcs.branch(destination)
        remote = config.default_remote
        remote_url_sha256 = vcs.assert_remote_url(
            destination, remote, remote_urls[name]
        )
        landed_ref = config.default_landed_ref
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
    vcs = GitVcs()
    checkouts = _existing_checkouts(config, args, vcs)
    slot_path = _assert_slot_contents(
        config,
        ActiveRecord(
            slot=args.slot,
            agent=args.agent,
            task=args.task,
            purpose=args.purpose,
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
    del slot_path
    for checkout in checkouts:
        _assert_checkout_identity_unchanged(config, checkout, vcs)
    confirmed_owner = _read_process_identity(owner.pid)
    if confirmed_owner != owner:
        raise Refusal("owner process generation changed during registration")
    _assert_caller_process(confirmed_owner, "owner")
    confirmed_coordinator = _read_process_identity(coordinator_lease.pid)
    if confirmed_coordinator != coordinator_lease:
        raise Refusal("coordinator process generation changed during registration")
    _assert_caller_process(confirmed_coordinator, "coordinator")
    now = _utc_now()
    return ActiveRecord(
        slot=args.slot,
        agent=args.agent,
        task=args.task,
        purpose=args.purpose,
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


def _cmd_register(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    owner = _capture_caller_process(args.owner_pid, "owner")
    coordinator_lease = _capture_caller_process(
        args.coordinator_pid, "coordinator"
    )
    with _mutation_locks(config, args.wait_lock):
        _refuse_partial_state(config)
        _assert_no_journal(config)
        states, archives = _validate_global_state(config)
        _assert_registry_storage_consistent(
            config, states, allowed_unregistered_slot=args.slot
        )
        before = _global_rows(states, archives)
        _ensure_state_shard(config)
        state = _load_active(config)
        _assert_agent_and_slot_free(config, args.slot, args.agent)
        record = _register_existing(
            config, args, owner, coordinator_lease
        )
        _atomic_write_json(_active_path(config), _active_to_obj(_append_record(state, record)))
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


def _cmd_import_existing(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    vcs = GitVcs()
    _validate_name(args.slot, "slot")
    checkouts = _existing_checkouts(config, args, vcs)
    print(
        f"verified existing slot={args.slot} checkouts={len(checkouts)} "
        f"machine={config.machine}"
    )
    for checkout in checkouts:
        print(
            f"  {checkout.name}: path={checkout.path} branch={checkout.branch} head={checkout.head}"
        )
    if not args.apply:
        print("dry-run: no state changed; add --apply --verified-live --owner-pid PID to register")
        return 0
    if args.owner_pid is None or args.coordinator_pid is None or not args.verified_live:
        raise Refusal(
            "--apply requires --verified-live, --owner-pid, and --coordinator-pid"
        )
    return _cmd_register(args)


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
        _assert_registry_storage_consistent(config, states)
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
        _atomic_write_json(
            _active_path(config), _active_to_obj(_replace_record(state, adopted))
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
        _assert_registry_storage_consistent(config, states)
        before = _global_rows(states, archives)
        state = _load_active(config)
        record = _find_record(state, args.slot)
        _expected_generation(record, args.expected_generation)
        if coordinator != record.coordinator_lease:
            raise Refusal("coordinator recovery requires the recorded coordinator process")
        if record.owner is not None:
            raise Refusal(
                f"slot {record.slot} has a historical owner; coordinator recovery cannot replace it"
            )
        _assert_registered_liveness(config, record)
        _slot_path, final_checkouts = _handoff_preconditions(
            config, record, GitVcs()
        )
        continuation = (
            f"wrkslots remove {record.slot} --coordinator-pid "
            f"{record.coordinator_lease.pid} --expected-generation {record.generation}"
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
        _atomic_write_json(
            _active_path(config), _active_to_obj(_replace_record(state, updated))
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
        _assert_registry_storage_consistent(config, states)
        before = _global_rows(states, archives)
        state = _load_active(config)
        record = _find_record(state, args.slot)
        _assert_owner_auth(record, args.agent, owner, args.expected_generation)
        updated = dataclasses.replace(record, heartbeat_at=_utc_now())
        _atomic_write_json(
            _active_path(config), _active_to_obj(_replace_record(state, updated))
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


def _status_record(record: ActiveRecord) -> dict[str, object]:
    process_state, process_detail = _process_state(record.owner)
    age, expired = _heartbeat_diagnosis(record)
    value = _record_to_obj(record)
    value["owner_state"] = process_state
    value["owner_detail"] = process_detail
    value["heartbeat_age_seconds"] = int(age)
    value["heartbeat_expired"] = expired
    return value


def _cmd_status(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    with _locked(
        config.worktrees / "ACTIVE", exclusive=False, wait_seconds=args.wait_lock
    ):
        _refuse_partial_state(config)
        all_states, _archives = _validate_global_state(config)
        _assert_registry_storage_consistent(config, all_states)
        states = (
            all_states
            if args.all_machines
            else [state for state in all_states if state.machine == config.machine]
        )
        journals = [path.name for path in _outstanding_journals(config)]
        records = [
            _status_record(record)
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
        f"project={config.root} worktrees={config.worktrees} "
        f"active={len(records)} journals={len(journals)}"
    )
    if journals:
        for journal in journals:
            print(f"RECOVERY REQUIRED: {journal}")
    for value in records:
        owner_state = _as_str(value["owner_state"], "status owner_state")
        expired = value["heartbeat_expired"] is True
        print(
            f"{value['machine']}/{value['slot']}: agent={value['agent']} task={value['task']} "
            f"generation={value['generation']} owner={owner_state} "
            f"heartbeat_age={value['heartbeat_age_seconds']}s expired={'yes' if expired else 'no'}"
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
) -> dict[str, object]:
    archive_id = f"{config.machine}:{record.slot}:{record.generation}:{finished_at}"
    fenced = (
        config.worktrees
        / f".{record.slot}.fenced.{record.generation}.{uuid.uuid4().hex}"
        / record.slot
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
        "record": _record_to_obj(record),
    }


def _archive_entry(
    journal: Mapping[str, object], record: ActiveRecord
) -> dict[str, object]:
    if record.handoff is None:
        raise StateError("cannot archive a slot without its owner-alive handoff")
    return {
        "archive_id": _as_str(journal["archive_id"], "journal.archive_id"),
        "slot": record.slot,
        "agent": record.agent,
        "task": record.task,
        "purpose": record.purpose,
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
        "checkouts": [_checkout_to_obj(item) for item in record.checkouts],
    }


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
            return archive
    updated = ArchiveState(
        machine=archive.machine,
        revision=archive.revision + 1,
        records=(*archive.records, entry),
    )
    _atomic_write_json(_archive_path(config), _archive_to_obj(updated))
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
    if (
        fenced.name != record.slot
        or fenced.parent.parent != config.worktrees
        or not fenced.parent.name.startswith(prefix)
        or len(fenced.parent.name) != len(prefix) + 32
        or not re.fullmatch(r"[0-9a-f]{32}", fenced.parent.name.removeprefix(prefix))
    ):
        raise StateError("finish journal fenced path does not match its slot generation")
    return fenced


def _checkout_at_slot(
    config: Config, checkout: Checkout, slot_path: Path
) -> tuple[Checkout, Path]:
    path = slot_path / checkout.name
    relative = path.relative_to(config.root).as_posix()
    return dataclasses.replace(checkout, path=relative), path


def _repair_checkout_at_slot(
    config: Config, checkout: Checkout, slot_path: Path, vcs: GitVcs
) -> Path:
    moved, path = _checkout_at_slot(config, checkout, slot_path)
    _repair_registration_at_slot(config, checkout, slot_path, vcs)
    current = _assert_checkout_safe(config, moved, vcs)
    if dataclasses.replace(current, path=checkout.path) != checkout:
        raise Refusal(
            f"fenced checkout {checkout.name} changed after finish was prepared; preserve it for inspection"
        )
    return path


def _repair_registration_at_slot(
    config: Config, checkout: Checkout, slot_path: Path, vcs: GitVcs
) -> Path:
    _moved, path = _checkout_at_slot(config, checkout, slot_path)
    _relative, repository = _repository_path(config, checkout.repository)
    vcs.repair_worktree(repository, path)
    return path


def _remove_fenced_directory(config: Config, fenced_slot: Path) -> None:
    if fenced_slot.exists():
        try:
            fenced_slot.rmdir()
        except OSError as exc:
            raise Refusal(
                f"fenced slot directory is not empty after Git removal: {fenced_slot}: {exc}"
            ) from exc
    try:
        fenced_slot.parent.rmdir()
    except FileNotFoundError:
        pass
    except OSError as exc:
        raise Refusal(f"cannot remove fenced slot parent {fenced_slot.parent}: {exc}") from exc
    _fsync_directory(config.worktrees)


def _rollback_path_fence(
    config: Config,
    record: ActiveRecord,
    journal: dict[str, object],
    vcs: GitVcs,
) -> None:
    removed = {
        _as_str(item, "journal.removed item")
        for item in _as_list(journal["removed"], "journal.removed")
    }
    if removed:
        raise Refusal("cannot roll back a path fence after a checkout was removed")
    original = config.worktrees / record.slot
    fenced = _finish_fenced_slot(config, record, journal)
    if original.exists() or original.is_symlink():
        raise Refusal(f"canonical slot reappeared before path-fence rollback: {original}")
    if not fenced.is_dir() or fenced.is_symlink():
        raise Refusal(f"fenced slot is missing or unsafe during rollback: {fenced}")
    try:
        os.rename(fenced, original)
        _fsync_directory(config.worktrees)
    except OSError as exc:
        raise Refusal(f"cannot restore path fence {fenced} to {original}: {exc}") from exc
    journal["phase"] = "prepared"
    _atomic_write_json(_journal_path(config), journal)
    for checkout in record.checkouts:
        _repair_checkout_at_slot(config, checkout, original, vcs)
    _remove_fenced_directory(config, fenced)
    _remove_control_file(_journal_path(config))


def _begin_or_resume_path_fence(
    config: Config,
    record: ActiveRecord,
    journal: dict[str, object],
    vcs: GitVcs,
) -> tuple[dict[str, object], Path]:
    original = config.worktrees / record.slot
    fenced = _finish_fenced_slot(config, record, journal)
    original_present = original.exists() or original.is_symlink()
    fenced_present = fenced.exists() or fenced.is_symlink()
    if original_present and fenced_present:
        raise Refusal(f"both canonical and fenced slot paths exist for {record.slot}")
    if original_present:
        removed = _as_list(journal["removed"], "journal.removed")
        if removed:
            raise StateError("finish journal records removals while the canonical slot still exists")
        _assert_slot_contents(config, record)
        for checkout in record.checkouts:
            current = _assert_checkout_safe(config, checkout, vcs)
            if current != checkout:
                raise Refusal(
                    f"checkout {checkout.name} changed after finish was prepared; preserve it for inspection"
                )
        _assert_slot_unused(original, record)
        try:
            fenced.parent.mkdir(mode=0o700)
            _fsync_directory(config.worktrees)
        except FileExistsError:
            if not fenced.parent.is_dir() or fenced.parent.is_symlink():
                raise Refusal(f"fenced slot parent is unsafe: {fenced.parent}")
            if any(fenced.parent.iterdir()):
                raise Refusal(f"fenced slot parent is not empty: {fenced.parent}")
        except OSError as exc:
            raise Refusal(f"cannot prepare fenced slot parent {fenced.parent}: {exc}") from exc
        try:
            os.rename(original, fenced)
            _fsync_directory(config.worktrees)
            _fsync_directory(fenced.parent)
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
        path = fenced / checkout.name
        if path.exists() or path.is_symlink():
            _repair_registration_at_slot(config, checkout, fenced, vcs)
    journal["phase"] = "fenced"
    _atomic_write_json(_journal_path(config), journal)
    _interrupt_for_test("after-path-fence")
    return journal, fenced


def _finish_remove_paths(
    config: Config,
    record: ActiveRecord,
    journal: dict[str, object],
    vcs: GitVcs,
) -> dict[str, object]:
    removed_values = _as_list(journal["removed"], "journal.removed")
    removed = {_as_str(item, "journal.removed item") for item in removed_values}
    all_names = {item.name for item in record.checkouts}
    if not removed <= all_names:
        raise StateError("finish journal names a checkout that is not in the active record")
    journal, fenced_slot = _begin_or_resume_path_fence(config, record, journal, vcs)
    present: list[Checkout] = []
    missing_after_remove: list[Checkout] = []
    for checkout in record.checkouts:
        if checkout.name in removed:
            continue
        _moved, path = _checkout_at_slot(config, checkout, fenced_slot)
        if path.is_symlink():
            raise Refusal(f"interrupted fenced checkout became a symlink: {path}")
        if path.exists():
            present.append(checkout)
            continue
        _relative, repository = _repository_path(config, checkout.repository)
        canonical = _stored_path(config, checkout.path, "checkout path")
        listed = vcs.listed_worktrees(repository)
        if path.absolute() in listed or canonical.absolute() in listed:
            raise Refusal(
                f"interrupted checkout {checkout.name} is missing but Git still registers it"
            )
        missing_after_remove.append(checkout)
    if missing_after_remove:
        removed.update(item.name for item in missing_after_remove)
        journal["removed"] = sorted(removed)
        _atomic_write_json(_journal_path(config), journal)
    remaining = tuple(present)
    if remaining:
        try:
            actual = {entry.name for entry in fenced_slot.iterdir()}
        except OSError as exc:
            raise Refusal(f"cannot inspect fenced slot {fenced_slot}: {exc}") from exc
        expected = {item.name for item in remaining}
        if actual != expected:
            raise Refusal(
                f"fenced slot contents changed: expected {sorted(expected)}, found {sorted(actual)}"
            )
        try:
            for checkout in remaining:
                _repair_checkout_at_slot(config, checkout, fenced_slot, vcs)
            _assert_slot_unused(fenced_slot, record)
        except Refusal as exc:
            if not removed:
                try:
                    _rollback_path_fence(config, record, journal, vcs)
                except Refusal as rollback:
                    raise Refusal(
                        f"{exc}; path-fence rollback failed: {rollback}; run 'wrkslots recover'"
                    ) from rollback
            raise
    elif fenced_slot.exists() and any(fenced_slot.iterdir()):
        raise Refusal(f"finished fenced slot contains unexpected files: {fenced_slot}")
    for checkout in remaining:
        _relative, repository = _repository_path(config, checkout.repository)
        _moved, path = _checkout_at_slot(config, checkout, fenced_slot)
        vcs.remove_worktree(repository, path)
        _interrupt_for_test("after-remove-before-journal")
        removed.add(checkout.name)
        journal["removed"] = sorted(removed)
        _atomic_write_json(_journal_path(config), journal)
        _interrupt_for_test("after-remove-worktree")
    _remove_fenced_directory(config, fenced_slot)
    journal["phase"] = "removed"
    _atomic_write_json(_journal_path(config), journal)
    return journal


def _assert_physical_slot_removed(
    config: Config,
    record: ActiveRecord,
    vcs: GitVcs,
    journal: Mapping[str, object],
) -> None:
    slot_path = config.worktrees / record.slot
    fenced_slot = _finish_fenced_slot(config, record, journal)
    if slot_path.exists() or slot_path.is_symlink():
        raise Refusal(
            f"cannot archive slot {record.slot}: physical slot still exists at {slot_path}"
        )
    if fenced_slot.exists() or fenced_slot.is_symlink() or fenced_slot.parent.exists():
        raise Refusal(
            f"cannot archive slot {record.slot}: fenced storage still exists at {fenced_slot.parent}"
        )
    for checkout in record.checkouts:
        path = _stored_path(config, checkout.path, "checkout path")
        if path.exists() or path.is_symlink():
            raise Refusal(
                f"cannot archive slot {record.slot}: checkout still exists at {path}"
            )
        _relative, repository = _repository_path(config, checkout.repository)
        _moved, fenced_path = _checkout_at_slot(config, checkout, fenced_slot)
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
    _assert_physical_slot_removed(config, record, GitVcs(), journal)
    archive = _load_archive(config)
    entry = _archive_entry(journal, record)
    _append_archive_once(config, archive, entry)
    _interrupt_for_test("after-archive-before-active")
    if current is not None:
        updated_state = _delete_record(state, record.slot)
        _atomic_write_json(_active_path(config), _active_to_obj(updated_state))
    _remove_control_file(_journal_path(config))


def _begin_finish(
    config: Config,
    state: ActiveState,
    record: ActiveRecord,
    *,
    mode: str,
    actor: str,
) -> None:
    vcs = GitVcs()
    _load_archive(config)
    _slot_path, final_checkouts = _remove_preconditions(config, record, vcs)
    final_record = dataclasses.replace(record, checkouts=final_checkouts)
    state = _replace_record(state, final_record)
    _atomic_write_json(_active_path(config), _active_to_obj(state))
    finished_at = _utc_now()
    journal = _finish_journal_payload(
        config,
        final_record,
        mode=mode,
        actor=actor,
        finished_at=finished_at,
        removed=(),
        phase="prepared",
    )
    _atomic_write_json(_journal_path(config), journal)
    _interrupt_for_test("after-finish-journal")
    journal = _finish_remove_paths(config, final_record, journal, vcs)
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
        _assert_registry_storage_consistent(config, states)
        before = _global_rows(states, archives)
        state = _load_active(config)
        record = _find_record(state, args.slot)
        _assert_owner_auth(record, args.agent, owner, args.expected_generation)
        if record.handoff is not None:
            raise Refusal(f"slot {record.slot} already has a recorded handoff")
        _slot_path, final_checkouts = _handoff_preconditions(
            config, record, GitVcs()
        )
        continuation = (
            f"wrkslots remove {record.slot} --coordinator-pid "
            f"{record.coordinator_lease.pid} --expected-generation {record.generation}"
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
        _atomic_write_json(
            _active_path(config), _active_to_obj(_replace_record(state, updated))
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
        _assert_registry_storage_consistent(config, states)
        before = _global_rows(states, archives)
        state = _load_active(config)
        record = _find_record(state, args.slot)
        _expected_generation(record, args.expected_generation)
        if coordinator != record.coordinator_lease:
            raise Refusal(
                f"coordinator process generation mismatch for slot {record.slot}"
            )
        _assert_caller_process(coordinator, "coordinator")
        if record.handoff is None:
            raise Refusal(f"slot {record.slot} has no recorded handoff")
        _assert_registered_liveness(config, record)
        owner_state, detail = _process_state(record.owner)
        if record.owner is None and record.coordinator_recovery_note is None:
            raise Refusal(
                f"slot {record.slot} has no owner lease and no coordinator recovery evidence"
            )
        if record.owner is not None and owner_state != "dead":
            raise Refusal(
                f"remove requires a proven-dead recorded owner; owner is {owner_state}: {detail}. "
                "TTL expiry is only a reason to inspect"
            )
        _begin_finish(config, state, record, mode="remove", actor="coordinator")
        after_states, after_archives = _validate_global_state(config)
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            args.slot,
        )
    print(f"removed and archived slot={record.slot} generation={record.generation}")
    return 0


def _recover_partial_updates(config: Config, discard: bool) -> bool:
    leftovers = sorted(config.worktrees.glob("ACTIVE.*.json.tmp.*"))
    leftovers += sorted(config.worktrees.glob("ARCHIVED.*.json.tmp.*"))
    leftovers += sorted(config.worktrees.glob("ACTIVE.*.journal.tmp.*"))
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
            _load_active(config, active_match.group(1))
        elif archive_match is not None:
            _load_archive(config, archive_match.group(1))
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


def _recover_create(
    config: Config, path: Path, raw: Mapping[str, object], state: ActiveState
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
    _exact_keys(raw, required, set(), "create journal")
    machine = _as_str(raw["machine"], "create journal.machine")
    if machine != config.machine or path != _journal_path(config, machine):
        raise StateError(
            f"create journal filename does not match machine {config.machine}"
        )
    slot = _as_str(raw["slot"], "create journal.slot")
    agent = _as_str(raw["agent"], "create journal.agent")
    _validate_name(slot, "slot")
    _validate_name(agent, "agent")
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
    planned_by_name = {item.name: item for item in plan}
    for item in plan:
        expected_destination, _destination = _checkout_path(config, slot, item.name)
        if item.destination != expected_destination:
            raise StateError(
                f"create journal destination for {item.name} does not match slot {slot}"
            )
        normalized_repository, _repository = _repository_path(config, item.repository)
        if item.repository != normalized_repository:
            raise StateError(
                f"create journal repository for {item.name} is not canonical"
            )
        if item.remote != config.default_remote or item.landed_ref != config.default_landed_ref:
            raise StateError(
                f"create journal target for {item.name} differs from configured authority"
            )
        if GitVcs().remote_url_sha256(_repository, item.remote) != item.remote_url_sha256:
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
        expected_record = ActiveRecord(
            slot=slot,
            agent=agent,
            task=task,
            purpose=purpose,
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
            _relative, repository = _repository_path(config, checkout.repository)
            destination = _stored_path(config, checkout.path, "checkout path")
            head = GitVcs().verify_existing_worktree(repository, destination)
            if head != checkout.head:
                raise Refusal(
                    f"create recovery found changed HEAD for {checkout.name}; preserve it for inspection"
                )
        _remove_control_file(path)
        print(f"recovered create: active state was durable; cleared journal for {slot}")
        return
    _assert_agent_and_slot_free(config, slot, agent)
    slot_path = config.worktrees / slot
    if slot_path.exists() and (not slot_path.is_dir() or slot_path.is_symlink()):
        raise Refusal(f"create recovery found an unsafe slot path: {slot_path}")
    if not slot_path.exists():
        slot_path.mkdir(mode=0o755)
        _fsync_directory(config.worktrees)
    vcs = GitVcs()
    created: list[Checkout] = []
    for item in plan:
        destination = _stored_path(config, item.destination, "checkout destination")
        _relative, repository = _repository_path(config, item.repository)
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
        _atomic_write_json(path, updated_journal)
    actual = {entry.name for entry in slot_path.iterdir()}
    expected = {item.name for item in plan}
    if actual != expected:
        raise Refusal(
            f"create recovery found unexpected slot contents: expected {sorted(expected)}, "
            f"found {sorted(actual)}"
        )
    record = ActiveRecord(
        slot=slot,
        agent=agent,
        task=task,
        purpose=purpose,
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
    _atomic_write_json(
        _active_path(config), _active_to_obj(_append_record(state, record))
    )
    _remove_control_file(path)
    print(f"recovered create: registered slot={slot} checkouts={len(created)}")


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
    _exact_keys(raw, required, set(), "finish journal")
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
        _assert_physical_slot_removed(config, record, GitVcs(), raw)
        _remove_control_file(path)
        print(f"recovered finish: cleared completed journal for {record.slot}")
        return
    if current is None:
        raise StateError("finish journal has neither its active record nor a durable archive entry")
    if _record_to_obj(current) != _record_to_obj(record):
        raise StateError("finish journal record does not exactly match ACTIVE")
    if coordinator != current.coordinator_lease:
        raise Refusal(
            f"coordinator process generation mismatch for slot {current.slot}"
        )
    _assert_caller_process(coordinator, "coordinator")
    _assert_registered_liveness(config, current)
    owner_state, detail = _process_state(current.owner)
    if current.owner is None and current.coordinator_recovery_note is None:
        raise Refusal(
            f"slot {current.slot} has no owner lease and no coordinator recovery evidence"
        )
    if current.owner is not None and owner_state != "dead":
        raise Refusal(f"recorded owner is {owner_state}: {detail}")
    journal = dict(raw)
    phase = _as_str(journal["phase"], "finish journal.phase")
    if phase not in ("prepared", "fenced", "removed"):
        raise StateError(f"unknown finish journal phase {phase!r}")
    if phase in ("prepared", "fenced"):
        journal = _finish_remove_paths(config, record, journal, GitVcs())
    else:
        if set(removed_names) != {checkout.name for checkout in record.checkouts}:
            raise StateError("removed finish phase does not name every checkout")
        _assert_physical_slot_removed(config, record, GitVcs(), journal)
    _finish_state_update(config, state, record, journal)
    print(f"recovered finish: archived and removed slot={record.slot}")


def _cmd_recover(args: argparse.Namespace) -> int:
    config = _load_config(args.project_root, args.machine)
    coordinator = _capture_caller_process(args.coordinator_pid, "coordinator")
    with _mutation_locks(config, args.wait_lock):
        recovered_partial = _recover_partial_updates(config, args.discard_partial)
        if recovered_partial and not _outstanding_journals(config):
            states, _archives = _validate_global_state(config)
            _assert_registry_storage_consistent(config, states)
            return 0
        path, raw = _load_journal(config)
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
            journal_coordinator = _identity_from_obj(
                raw.get("coordinator_lease"), "create journal.coordinator_lease"
            )
            if journal_coordinator != coordinator:
                raise Refusal("create recovery requires the recorded coordinator process")
            _recover_create(config, path, raw, state)
        elif kind == "finish":
            _recover_finish(config, path, raw, state, coordinator)
        else:
            raise StateError(f"unknown recovery journal kind {kind!r}")
        after_states, after_archives = _validate_global_state(config)
        slot = _as_str(raw.get("slot"), "journal.slot")
        _assert_only_slot_changed(
            before,
            _global_rows(after_states, after_archives),
            slot,
        )
    return 0


class _HelpFormatter(argparse.RawDescriptionHelpFormatter):
    def __init__(self, prog: str) -> None:
        super().__init__(prog, max_help_position=30, width=100)


def _add_repo_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--repo",
        action="append",
        metavar="NAME=PATH",
        help="source Git repository relative to the project root (repeatable)",
    )
    parser.add_argument(
        "--remote-url",
        action="append",
        required=True,
        metavar="NAME=URL",
        help="trusted fetch URL supplied during provisioning (repeatable)",
    )


def _build_parser() -> argparse.ArgumentParser:
    description = """Safely manage opaque Git worktree slots with one active slot per agent.

Read-only: status and import-existing without --apply.
Mutating: init, create, register, import-existing --apply, adopt,
recover-unbound-owner, heartbeat, finish, remove, and recover. Every mutating command takes a state lock and
atomically replaces the affected machine shard.
"""
    epilog = """Lifecycle:
  wrkslots init --liveness-command ci-hub/health/agent_liveness_probe.py
  wrkslots create slot01 --agent codex-1 --task task-123 --purpose "fix parser" \\
    --repo product=product --remote-url product=URL --branch product=codex/fix-parser \\
    --coordinator-pid PID --owner-pid PID
  wrkslots heartbeat slot01 --agent codex-1 --owner-pid PID --expected-generation 1
  wrkslots finish slot01 --agent codex-1 --owner-pid PID --expected-generation 1 \\
    --validation "exact command and result"

finish proves clean index/tracked/untracked/ignored state, no unfinished Git
operation, remote durability, exact landed ancestry, and path identity, then
records the owner-alive handoff while retaining physical storage. remove is
coordinator-only and proceeds only after the registered liveness command
returns rc 0, the recorded owner process generation is dead, and independent
process/cgroup/mount checks prove the slot unused. rc 1 and rc 2 both refuse.
A stale heartbeat or expired TTL is diagnostic only.

Any failed precondition leaves every worktree in place. If a process stops
during create or remove, later mutations refuse and 'wrkslots recover' resumes
from the durable journal. Never edit ACTIVE.*, ARCHIVED.*, or a journal by hand.

Global options (--project-root, --machine, --wait-lock) precede the command.
Exit status: 0 success, 2 command-line usage, 3 fail-closed refusal.
"""
    parser = argparse.ArgumentParser(
        prog="wrkslots",
        description=description,
        epilog=epilog,
        formatter_class=_HelpFormatter,
    )
    parser.add_argument("--version", action="version", version=f"wrkslots {VERSION}")
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
    subparsers = parser.add_subparsers(dest="command", metavar="COMMAND")

    init = subparsers.add_parser(
        "init",
        help="initialize config, state shards, directory, and relative command symlink",
        formatter_class=_HelpFormatter,
    )
    init.add_argument("directory", nargs="?", default=".", help="project root (default: .)")
    init.add_argument(
        "--worktrees-dir", default="worktrees", help="relative opaque directory (default: worktrees)"
    )
    init.add_argument("--default-remote", default="origin")
    init.add_argument("--default-landed-ref", default="refs/remotes/origin/main")
    init.add_argument("--heartbeat-ttl-seconds", type=int, default=3600)
    init.add_argument(
        "--liveness-command",
        required=True,
        help="executable path relative to the project root; rc 0 dead, 1 alive, 2 unverifiable",
    )
    init.set_defaults(handler=_cmd_init)

    status = subparsers.add_parser("status", help="show active ownership and diagnoses")
    status.add_argument("--all-machines", action="store_true", help="read every ACTIVE shard")
    status.add_argument("--slot", help="show one slot")
    status.add_argument("--format", choices=("human", "json"), default="human")
    status.set_defaults(handler=_cmd_status)

    create = subparsers.add_parser(
        "create", help="create fresh Git worktrees and register one active owner"
    )
    create.add_argument("slot")
    create.add_argument("--agent", required=True)
    create.add_argument("--task", required=True)
    create.add_argument("--purpose", required=True)
    create.add_argument(
        "--owner-pid",
        type=int,
        help="live owner PID to bind; omit only when the owner will immediately adopt",
    )
    create.add_argument("--coordinator-pid", type=int, required=True)
    _add_repo_options(create)
    create.add_argument(
        "--branch", action="append", required=True, metavar="NAME=BRANCH"
    )
    create.add_argument(
        "--start", action="append", metavar="NAME=REF", help="default: configured landed ref"
    )
    create.set_defaults(handler=_cmd_create)

    register = subparsers.add_parser(
        "register", help="register already-created, verified live Git worktrees"
    )
    register.add_argument("slot")
    register.add_argument("--agent", required=True)
    register.add_argument("--task", required=True)
    register.add_argument("--purpose", required=True)
    register.add_argument("--owner-pid", type=int, required=True)
    register.add_argument("--coordinator-pid", type=int, required=True)
    register.add_argument("--verified-live", action="store_true", required=True)
    _add_repo_options(register)
    register.set_defaults(handler=_cmd_register)

    import_existing = subparsers.add_parser(
        "import-existing",
        help="verify an existing slot and print the registration; dry-run by default",
    )
    import_existing.add_argument("slot")
    import_existing.add_argument("--agent", required=True)
    import_existing.add_argument("--task", required=True)
    import_existing.add_argument("--purpose", required=True)
    import_existing.add_argument("--owner-pid", type=int)
    import_existing.add_argument("--coordinator-pid", type=int)
    import_existing.add_argument("--verified-live", action="store_true")
    import_existing.add_argument("--apply", action="store_true")
    _add_repo_options(import_existing)
    import_existing.set_defaults(handler=_cmd_import_existing)

    adopt = subparsers.add_parser("adopt", help="bind a live owner process generation")
    adopt.add_argument("slot")
    adopt.add_argument("--agent", required=True)
    adopt.add_argument("--owner-pid", type=int, required=True)
    adopt.add_argument("--expected-generation", type=int, required=True)
    adopt.set_defaults(handler=_cmd_adopt)

    recover_unbound = subparsers.add_parser(
        "recover-unbound-owner",
        help="record coordinator recovery evidence without replacing historical ownership",
    )
    recover_unbound.add_argument("slot")
    recover_unbound.add_argument("--coordinator-pid", type=int, required=True)
    recover_unbound.add_argument("--expected-generation", type=int, required=True)
    recover_unbound.add_argument("--recovery-note", required=True)
    recover_unbound.add_argument("--validation", action="append", required=True)
    recover_unbound.add_argument("--limitation", action="append")
    recover_unbound.set_defaults(handler=_cmd_recover_unbound_owner)

    heartbeat = subparsers.add_parser("heartbeat", help="update a bound owner's heartbeat")
    heartbeat.add_argument("slot")
    heartbeat.add_argument("--agent", required=True)
    heartbeat.add_argument("--owner-pid", type=int, required=True)
    heartbeat.add_argument("--expected-generation", type=int, required=True)
    heartbeat.set_defaults(handler=_cmd_heartbeat)

    finish = subparsers.add_parser(
        "finish", help="prove landed safety and record the owner-alive handoff"
    )
    finish.add_argument("slot")
    finish.add_argument("--agent", required=True)
    finish.add_argument("--owner-pid", type=int, required=True)
    finish.add_argument("--expected-generation", type=int, required=True)
    finish.add_argument("--validation", action="append", required=True)
    finish.add_argument("--limitation", action="append")
    finish.set_defaults(handler=_cmd_finish)

    remove = subparsers.add_parser(
        "remove", help="finish a slot only after its recorded owner is proven dead"
    )
    remove.add_argument("slot")
    remove.add_argument("--coordinator-pid", type=int, required=True)
    remove.add_argument("--expected-generation", type=int, required=True)
    remove.set_defaults(handler=_cmd_remove)

    recover = subparsers.add_parser(
        "recover", help="resume one interrupted create or removal from its journal"
    )
    recover.add_argument("--coordinator-pid", type=int, required=True)
    recover.add_argument(
        "--discard-partial",
        action="store_true",
        help="discard an incomplete temp write only when its prior durable file is valid",
    )
    recover.set_defaults(handler=_cmd_recover)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    values = list(sys.argv[1:] if argv is None else argv)
    if not values:
        parser.print_help()
        return 0
    args = parser.parse_args(values)
    if args.wait_lock < 0 or not args.wait_lock < float("inf"):
        parser.error("--wait-lock must be a finite non-negative number")
    if args.command == "init":
        if args.heartbeat_ttl_seconds <= 0:
            parser.error("--heartbeat-ttl-seconds must be positive")
        if args.project_root is not None:
            parser.error("init uses its DIRECTORY argument, not --project-root")
    handler = getattr(args, "handler", None)
    if handler is None:
        parser.print_help()
        return 0
    try:
        result = handler(args)
    except Refusal as exc:
        print(f"REFUSED: {exc}", file=sys.stderr)
        return 3
    return int(result)


if __name__ == "__main__":
    raise SystemExit(main())
