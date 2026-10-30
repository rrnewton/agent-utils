"""Where an agentctl registry keeps each agent, and moving it there (#593 registry-agents-subdir).

A registry holds a few fixed items and several collections that grow with the number of agents.
Every such collection has a subdirectory of its own, so the fixed items stay in plain sight
however many agents have come and gone (layout 2)::

    REGISTRY/
      profiles.json, coordinator.json, restore.json, landing-queue.json    fixed
      .identity.lock, .coordinator.lock, .restore.lock                     fixed, registry-wide
      agents/<name>/            one directory per active agent
      agents/agent.json         the layout tripwire; never an agent
      locks/<name>.lock         the agent's lifecycle lock
      locks/<name>.rank.lock    its rank companion, for ranked waiters
      archive/<name>-<token>/   retired agents
      .progress/, .waiting/, .renames/, .revives/, .inbox/                 other collections

Layout 1, written by every build before this module, kept each agent directory at
``REGISTRY/<name>/`` and its locks at ``REGISTRY/.<name>.lock`` and ``.<name>.rank.lock``. Readers
accept both until :data:`LEGACY_SUPPORT_ENDS`; new agents are always created in layout 2, and
``agentctl migrate-layout`` (also run automatically by commands that write) moves the rest.

The rules, shared by every agentctl edition:

* An agent lives in exactly one layout; a name with a directory in both is refused.
* Locks follow the directory: a layout-1 agent is locked where older builds lock it, so old and
  new builds still exclude each other; a caller re-locates the agent after taking its lock.
* ``agents/agent.json`` holds a JSON string, which every layout-1 build refuses as an unreadable
  record of an agent named ``agents``, and ``.renames/upgrade-agentctl-registry-layout-2``, which
  such a build refuses as an unexpected rename-journal entry before it starts, adopts, or loads
  any agent: old builds fail closed instead of misreading the registry.
* Migration moves one agent at a time by one no-replace rename, holding the agent's old and new
  lifecycle locks and the identity lock. An agent another process uses, whose record names paths
  inside its directory, or whose lock is busy, stays where it is.
"""
from __future__ import annotations

import fcntl
import json
import os
import re
import stat
import tempfile
import time
from collections.abc import Callable, Mapping
from pathlib import Path

from agentctl import agent
from agentctl.errors import AgentDeliveryError

AGENTS = "agents"
LOCKS = "locks"
TRIPWIRE = "agent.json"
TRIPWIRE_TEXT = (
    "agentctl registry layout 2: each agent lives in agents/<name>/ and its locks in locks/. "
    "This file is not an agent; it makes agentctl builds older than layout 2 refuse this "
    "registry instead of misreading it. Upgrade agentctl."
)
RENAMES_TRIPWIRE = "upgrade-agentctl-registry-layout-2"
RENAMES_TRIPWIRE_TEXT = (
    "This registry uses agentctl registry layout 2. agentctl builds older than layout 2 stop at "
    "this file; upgrade agentctl. Readers of rename journals skip it.\n"
)
#: The directory holding the registry's chat bridges, one entry each.
BRIDGES = "bridges"
RESERVED_NAMES = ("archive", AGENTS, LOCKS, BRIDGES)
#: Registry-wide ``.<stem>.lock`` files that are fixed items, not agents' layout-1 locks.
FIXED_LOCK_STEMS = ("identity", "coordinator", "restore")
LEGACY_SUPPORT_ENDS = "2026-11-26"
REMOVAL_ISSUE = "#597 registry-old-layout-removal"
AUTO_MIGRATE_VARIABLE = "AGENTCTL_LAYOUT_AUTO_MIGRATE"
CURRENT = "current"
LEGACY = "legacy"

_NAME = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")


def is_agent_name(value: str) -> bool:
    """An agent name of the shared on-disk formats that no registry directory reserves."""
    return bool(_NAME.fullmatch(value)) and value not in RESERVED_NAMES


def agents_directory(registry: Path) -> Path:
    """``REGISTRY/agents``, the directory holding one subdirectory per agent."""
    return registry / AGENTS


def locks_directory(registry: Path) -> Path:
    """``REGISTRY/locks``, the directory holding the agents' lifecycle locks."""
    return registry / LOCKS


def current_directory(registry: Path, name: str) -> Path:
    """An agent's directory in layout 2, whether or not it exists."""
    return registry / AGENTS / name


def legacy_directory(registry: Path, name: str) -> Path:
    """An agent's directory in layout 1, whether or not it exists."""
    return registry / name


def directory_at(registry: Path, name: str, location: str) -> Path:
    """The directory of agent ``name`` at ``location``: layout 2 or layout 1."""
    return current_directory(registry, name) if location == CURRENT else legacy_directory(registry, name)


def _present(path: Path) -> bool:
    try:
        os.stat(path, follow_symlinks=False)
    except FileNotFoundError:
        return False
    except OSError as exc:
        raise AgentDeliveryError(f"cannot inspect agent directory {path}: {exc}") from exc
    return True


def _stray(legacy: Path) -> bool:
    """A layout-1 directory with no record: what an older program writing into an agent's queue by
    path leaves after a migration. Beside a layout-2 agent it is not an agent."""
    try:
        metadata = os.stat(legacy, follow_symlinks=False)
    except FileNotFoundError:
        return False
    except OSError as exc:
        raise AgentDeliveryError(f"cannot inspect agent directory {legacy}: {exc}") from exc
    return stat.S_ISDIR(metadata.st_mode) and not _present(legacy / "agent.json")


def locate(registry: Path, name: str) -> str:
    """Layout 1 when anything is at ``REGISTRY/<name>``, otherwise layout 2 (where new agents go).

    A name with an agent in both layouts is refused; a layout-1 stray beside a layout-2 agent is
    ignored.
    """
    if name in RESERVED_NAMES:
        raise AgentDeliveryError(f"'{name}' is reserved and cannot name an agent")
    legacy = legacy_directory(registry, name)
    current = current_directory(registry, name)
    legacy_present, current_present = _present(legacy), _present(current)
    if legacy_present and current_present and _stray(legacy):
        return CURRENT
    if legacy_present and current_present:
        raise AgentDeliveryError(
            f"agent '{name}' has a directory in both registry layouts, {legacy} and {current}; "
            "an agentctl build older than layout 2 probably created the first after the second "
            "was migrated. Keep the one you want and move the other aside "
            "(see `agentctl migrate-layout --help`)"
        )
    return LEGACY if legacy_present else CURRENT


def directory(registry: Path, name: str) -> Path:
    """Agent ``name``'s directory in whichever layout holds it (see :func:`locate`)."""
    return directory_at(registry, name, locate(registry, name))


def lock_paths(registry: Path, name: str, location: str) -> tuple[Path, Path]:
    """The lifecycle lock and rank lock of agent ``name`` at ``location``."""
    if location == CURRENT:
        locks = locks_directory(registry)
        return locks / f"{name}.lock", locks / f"{name}.rank.lock"
    return registry / f".{name}.lock", registry / f".{name}.rank.lock"


def _entries(path: Path) -> list[str]:
    try:
        return os.listdir(path)
    except FileNotFoundError:
        return []
    except OSError as exc:
        raise AgentDeliveryError(f"cannot list {path}: {exc}") from exc


def legacy_names(registry: Path) -> list[str]:
    """Agent names with an entry in layout 1, sorted."""
    return sorted(name for name in _entries(registry) if is_agent_name(name))


def current_names(registry: Path) -> list[str]:
    """Agent names with an entry in layout 2, sorted."""
    return sorted(name for name in _entries(agents_directory(registry)) if is_agent_name(name))


def stray_names(registry: Path) -> list[str]:
    """Names whose layout-1 entry is a stray beside a layout-2 agent, sorted."""
    current = set(current_names(registry))
    return [name for name in legacy_names(registry)
            if name in current and _stray(legacy_directory(registry, name))]


def record_directories(registry: Path, name: str) -> list[Path]:
    """Every directory holding ``agent.json`` under ``name``, layout 2 first: two for a conflict."""
    return [directory for directory in (current_directory(registry, name), legacy_directory(registry, name))
            if _present(directory / "agent.json")]


def names(registry: Path) -> list[str]:
    """Every agent name in either layout, sorted, each once."""
    return sorted(set(legacy_names(registry)) | set(current_names(registry)))


def legacy_lock_files(registry: Path) -> list[tuple[str, Path]]:
    """``(agent, path)`` of every layout-1 lock file left in the top level, sorted by path."""
    found = []
    for entry in _entries(registry):
        if not (entry.startswith(".") and entry.endswith(".lock")):
            continue
        stem = entry[1:-len(".lock")]
        rank = stem.endswith(".rank")
        agent_name = stem[:-len(".rank")] if rank else stem
        if is_agent_name(agent_name) and (rank or agent_name not in FIXED_LOCK_STEMS):
            found.append((agent_name, registry / entry))
    return sorted(found, key=lambda item: str(item[1]))


def is_tripwire(path: Path) -> bool:
    """Whether ``path`` holds a JSON string, as the layout tripwire does."""
    try:
        return isinstance(json.loads(path.read_bytes()), str)
    except (OSError, ValueError):
        return False


def _create_private_directory(path: Path, purpose: str) -> None:
    try:
        os.mkdir(path, 0o700)
    except FileExistsError:
        pass
    except OSError as exc:
        raise AgentDeliveryError(f"cannot prepare {purpose} {path}: {exc}") from exc
    agent._validate_private_directory(str(path), purpose, tighten=True)


def _create_noreplace(path: Path, text: str) -> None:
    temporary = ""
    try:
        with tempfile.NamedTemporaryFile(
            "w", encoding="utf-8", dir=path.parent, prefix=".message.", delete=False
        ) as handle:
            temporary = handle.name
            os.fchmod(handle.fileno(), 0o600)
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        try:
            os.link(temporary, path)
        except FileExistsError:
            return
        agent._fsync_dir(str(path.parent))
    except OSError as exc:
        raise AgentDeliveryError(f"cannot create registry layout tripwire {path}: {exc}") from exc
    finally:
        if temporary:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


def prepare(registry: Path) -> None:
    """Create ``agents/``, ``locks/`` and both tripwires, whichever are missing."""
    registry.mkdir(mode=0o700, parents=True, exist_ok=True)
    agent._validate_private_directory(str(registry), "agent registry", tighten=True)
    # Builds before layout 2 allowed agents named `agents` and `locks`; such an agent's directory
    # is no place for a collection.
    for collection, record in ((AGENTS, agents_directory(registry) / TRIPWIRE),
                               (LOCKS, locks_directory(registry) / "agent.json")):
        if _present(record) and (collection == LOCKS or not is_tripwire(record)):
            raise AgentDeliveryError(
                f"{registry / collection} is the directory of a layout-1 agent named '{collection}', "
                "which layout 2 reserves; rename it with an older agentctl first"
            )
    agents = agents_directory(registry)
    _create_private_directory(agents, "agent directory collection")
    tripwire = agents / TRIPWIRE
    try:
        metadata = os.stat(tripwire, follow_symlinks=False)
    except FileNotFoundError:
        _create_noreplace(tripwire, json.dumps(TRIPWIRE_TEXT) + "\n")
    else:
        if not stat.S_ISREG(metadata.st_mode):
            raise AgentDeliveryError(f"unsafe registry layout tripwire: {tripwire}")
    renames = registry / ".renames"
    _create_private_directory(renames, "rename journal directory")
    if not os.path.lexists(renames / RENAMES_TRIPWIRE):
        _create_noreplace(renames / RENAMES_TRIPWIRE, RENAMES_TRIPWIRE_TEXT)
    _create_private_directory(locks_directory(registry), "agent lock collection")


def prepare_locks(registry: Path) -> None:
    """Create ``REGISTRY/locks`` (mode 0700) if it is missing."""
    _create_private_directory(locks_directory(registry), "agent lock collection")


def lock_still_current(registry: Path, name: str, location: str, descriptor: int, path: Path) -> bool:
    """Whether the lock just taken still guards the agent: not after a migration moved it."""
    if locate(registry, name) != location:
        return False
    held = os.fstat(descriptor)
    try:
        named = os.stat(path, follow_symlinks=False)
    except FileNotFoundError:
        return False
    return (named.st_dev, named.st_ino) == (held.st_dev, held.st_ino)


def _lock_within(descriptor: int, wait: float) -> bool:
    deadline = time.monotonic() + wait
    while True:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except BlockingIOError:
            if time.monotonic() >= deadline:
                return False
            time.sleep(0.02)


def _command(pid: int) -> str:
    try:
        raw = Path(f"/proc/{pid}/cmdline").read_bytes()
    except OSError:
        return ""
    return " ".join(word.decode(errors="replace") for word in raw.split(b"\0") if word)


def _working_inside(directory: Path) -> dict[str, object] | None:
    """A process other than this one working inside ``directory``. Only working directories are
    read: listing every process's open files costs seconds on a busy host, and the lock files in
    the directory cover the programs that keep it open."""
    try:
        canonical = os.path.realpath(directory, strict=True)
        processes = os.listdir("/proc")
    except OSError:
        return None
    own = str(os.getpid())
    for pid in processes:
        if pid == own or not pid.isdigit():
            continue
        try:
            cwd = os.readlink(f"/proc/{pid}/cwd")
        except OSError:
            continue
        if cwd == canonical or cwd.startswith(canonical + "/"):
            return {"pid": int(pid), "command": _command(int(pid))}
    return None


def _flock_holder(path: Path) -> int | None:
    """The process holding a ``flock`` on ``path``, from ``/proc/locks``."""
    try:
        metadata = os.stat(path)
        text = Path("/proc/locks").read_text(encoding="ascii", errors="replace")
    except OSError:
        return None
    for line in text.splitlines():
        fields = line.split()
        if len(fields) < 6 or fields[1] != "FLOCK" or fields[1] == "->":
            continue
        place = fields[5].split(":")
        if len(place) != 3 or not place[2].isdigit() or int(place[2]) != metadata.st_ino:
            continue
        pid = int(fields[4])
        # /proc/locks may print another device for some filesystems; accept a holder there only
        # when it has the file itself open.
        if (int(place[0], 16), int(place[1], 16)) == (os.major(metadata.st_dev), os.minor(metadata.st_dev)):
            return pid
        try:
            for descriptor in os.listdir(f"/proc/{pid}/fd"):
                opened = os.stat(f"/proc/{pid}/fd/{descriptor}")
                if (opened.st_dev, opened.st_ino) == (metadata.st_dev, metadata.st_ino):
                    return pid
        except OSError:
            continue
    return None


def _held_lock_inside(directory: Path) -> dict[str, object] | None:
    """A process holding a lock file in the agent directory or one level down (a headless
    wrapper's ``headless.lock``, a drain's ``queue/.delivery.lock``)."""
    candidates: list[Path] = []
    try:
        for entry in directory.iterdir():
            if entry.is_dir() and not entry.is_symlink():
                try:
                    candidates.extend(entry.iterdir())
                except OSError:
                    pass
            else:
                candidates.append(entry)
    except OSError:
        return None
    for path in candidates:
        if path.suffix == ".lock":
            pid = _flock_holder(path)
            if pid is not None:
                return {"pid": pid, "command": _command(pid)}
    return None


def _record_names_its_directory(path: Path) -> str | None:
    try:
        text = (path / "agent.json").read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None
    spellings = [str(path)]
    try:
        spellings.append(os.path.realpath(path, strict=True))
    except OSError:
        pass
    if any(f"{spelling}/" in text or f'{spelling}"' in text for spelling in spellings):
        return ("its record names paths inside its directory (a turn runner's runtime home); "
                "retire or restart it to move it")
    return None


def _unmovable(path: Path) -> str | None:
    user = _working_inside(path) or _held_lock_inside(path)
    if user is not None:
        return f"in use by process {user['pid']} ({user['command']})"
    return _record_names_its_directory(path)


def _is_fixed_lock(path: Path) -> bool:
    name = path.name
    return name.startswith(".") and name.endswith(".lock") and name[1:-len(".lock")] in FIXED_LOCK_STEMS


def _remove_lock_file(path: Path, held: int | None) -> bool | str:
    """True when removed, False when already gone, or why it was kept."""
    opened = -1
    try:
        if held is None:
            if not path.exists():
                return False
            try:
                opened = agent._open_private_lock(str(path), "layout-1 lock file")
            except AgentDeliveryError as exc:
                return str(exc)
            try:
                fcntl.flock(opened, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return "held by another process"
        try:
            os.unlink(path)
        except FileNotFoundError:
            return False
        except OSError as exc:
            return str(exc)
        return True
    finally:
        if opened >= 0:
            os.close(opened)


def _open_directory(path: Path) -> int:
    try:
        return os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW)
    except OSError as exc:
        raise AgentDeliveryError(f"cannot open {path}: {exc}") from exc


def _move_agent(registry: Path, name: str, wait: float,
                removed: list[str], kept: list[dict[str, object]]) -> str | None:
    from agentctl.subagents import _rename_directory_noreplace_at

    legacy = legacy_directory(registry, name)
    legacy_lock_path, legacy_rank_path = lock_paths(registry, name, LEGACY)
    current_lock_path, _ = lock_paths(registry, name, CURRENT)
    descriptors: list[int] = []
    try:
        legacy_lock = agent._open_private_lock(str(legacy_lock_path), "agent lifecycle lock")
        descriptors.append(legacy_lock)
        if not _lock_within(legacy_lock, wait):
            return "its lifecycle lock is busy"
        current_lock = agent._open_private_lock(str(current_lock_path), "agent lifecycle lock")
        descriptors.append(current_lock)
        if not _lock_within(current_lock, wait):
            return "its lifecycle lock is busy"
        # Layout 1 gave an agent named `identity` the registry identity lock as its own lock,
        # which this process already holds.
        if legacy_lock_path != registry / ".identity.lock":
            identity = agent._open_private_lock(str(registry / ".identity.lock"), "agent identity lock")
            descriptors.append(identity)
            if not _lock_within(identity, wait):
                return "the registry identity lock is busy"
        try:
            metadata = os.stat(legacy, follow_symlinks=False)
        except FileNotFoundError:
            return "it disappeared before it could be moved"
        if not stat.S_ISDIR(metadata.st_mode):
            return "it is not a directory"
        try:
            agent._validate_private_directory(str(legacy), "agent directory")
        except AgentDeliveryError as exc:
            return str(exc)
        reason = _unmovable(legacy)
        if reason is not None:
            return reason
        current = current_directory(registry, name)
        if _present(current):
            return f"{current} already exists; keep one of the two directories"
        source = _open_directory(registry)
        descriptors.append(source)
        destination = _open_directory(agents_directory(registry))
        descriptors.append(destination)
        try:
            _rename_directory_noreplace_at(source, name, destination, name)
        except AgentDeliveryError as exc:
            raise AgentDeliveryError(f"cannot move agent directory {legacy} to {current}: {exc}") from exc
        os.fsync(destination)
        os.fsync(source)
        for path, held in ((legacy_rank_path, None), (legacy_lock_path, legacy_lock)):
            if _is_fixed_lock(path):
                continue
            outcome = _remove_lock_file(path, held)
            if outcome is True:
                removed.append(str(path))
            elif isinstance(outcome, str):
                kept.append({"path": str(path), "reason": outcome})
        return None
    finally:
        for descriptor in reversed(descriptors):
            os.close(descriptor)


def _layout_of(legacy: int, current: int, legacy_locks: int) -> str:
    old, new = legacy > 0 or legacy_locks > 0, current > 0
    return {(False, False): "empty", (True, False): "1", (False, True): "2"}.get((old, new), "mixed")


def _legacy_agents(registry: Path) -> tuple[list[str], list[str]]:
    """The layout-1 names that are agents, and the strays, both sorted."""
    strays = stray_names(registry)
    return [name for name in legacy_names(registry) if name not in strays], strays


def migrate(registry: Path, *, apply: bool, wait: float) -> dict[str, object]:
    """Move every idle layout-1 agent to layout 2; without ``apply``, only report. Idempotent.

    The report holds ``moved`` (``would_move``), ``left``, ``conflicts``,
    ``strays``, ``removed_lock_files`` (``would_remove_lock_files``), ``kept_lock_files``.
    """
    moved_key, removed_key = ("moved", "removed_lock_files") if apply else ("would_move", "would_remove_lock_files")
    report: dict[str, object] = {
        "registry": str(registry), "apply": apply,
        "legacy_support_ends": LEGACY_SUPPORT_ENDS, "removal_issue": REMOVAL_ISSUE,
    }
    if not registry.exists():
        report.update({"layout_before": "empty", "layout": "empty", moved_key: [], "left": [],
                       "conflicts": [], "strays": [], removed_key: 0, "kept_lock_files": []})
        return report
    agent._validate_private_directory(str(registry), "agent registry")
    legacy, strays = _legacy_agents(registry)
    current = current_names(registry)
    legacy_locks = legacy_lock_files(registry)
    layout_before = _layout_of(len(legacy), len(current), len(legacy_locks))
    conflicts = [name for name in legacy if name in current]
    candidates = [name for name in legacy if name not in conflicts]
    moved: list[str] = []
    left: list[dict[str, object]] = []
    removed: list[str] = []
    kept: list[dict[str, object]] = []
    if apply:
        if candidates or current:
            prepare(registry)
        for name in candidates:
            # One agent's failure leaves that agent where it is, not the rest.
            try:
                reason = _move_agent(registry, name, wait, removed, kept)
            except (AgentDeliveryError, OSError) as exc:
                reason = str(exc)
            if reason is None:
                moved.append(name)
            else:
                left.append({"name": name, "reason": reason})
        for agent_name, path in legacy_lock_files(registry):
            try:
                still_legacy = locate(registry, agent_name) == LEGACY
            except AgentDeliveryError:
                still_legacy = False
            if still_legacy or agent_name in conflicts:
                continue
            outcome = _remove_lock_file(path, None)
            if outcome is True:
                removed.append(str(path))
            elif isinstance(outcome, str):
                kept.append({"path": str(path), "reason": outcome})
        report["layout"] = _layout_of(len(_legacy_agents(registry)[0]), len(current_names(registry)),
                                      len(legacy_lock_files(registry)))
        report[removed_key] = len(removed)
    else:
        for name in candidates:
            reason = _unmovable(legacy_directory(registry, name))
            if reason is None:
                moved.append(name)
            else:
                left.append({"name": name, "reason": reason})
        report["layout"] = layout_before
        report[removed_key] = sum(1 for _, path in legacy_locks if not _is_fixed_lock(path))
    report.update({"layout_before": layout_before, moved_key: moved, "left": left,
                   "conflicts": conflicts, "strays": strays, "kept_lock_files": kept})
    return report


def notice(report: Mapping[str, object]) -> str | None:
    """The one-line notice for an automatic migration, or None when nothing moved."""

    def listed(key: str) -> list[str]:
        values = report.get(key)
        if not isinstance(values, list):
            return []
        return [value if isinstance(value, str) else str(value.get("name")) for value in values
                if isinstance(value, (str, dict))]

    moved, left, conflicts = listed("moved"), listed("left"), listed("conflicts")
    if not moved:
        return None
    parts = [f"moved {len(moved)} agent(s) to {report['registry']}/agents/ ({', '.join(moved)})"]
    if left:
        parts.append(f"left {len(left)} in the old layout for now, in use or locked ({', '.join(left)})")
    if conflicts:
        parts.append(f"found {len(conflicts)} in both layouts, which needs a person ({', '.join(conflicts)})")
    return (f"agentctl: registry layout 2 (#593 registry-agents-subdir): {'; '.join(parts)}. "
            "Older agentctl builds cannot use this registry; the old layout stays readable until "
            f"{LEGACY_SUPPORT_ENDS}. See `agentctl migrate-layout --help`.")


def auto_migrate(registry: Path, environment: Callable[[str], str | None] = os.environ.get) -> str | None:
    """Before a command that writes: move idle layout-1 agents without waiting; never fails."""
    value = environment(AUTO_MIGRATE_VARIABLE)
    if value is not None and value.strip().lower() in ("0", "false", "no", "off"):
        return None
    if not registry.is_dir():
        return None
    try:
        if not _legacy_agents(registry)[0] and not legacy_lock_files(registry):
            return None
        report = migrate(registry, apply=True, wait=0.0)
    except (AgentDeliveryError, OSError) as exc:
        return (f"agentctl: could not move {registry} to registry layout 2 ({exc}); run "
                "`agentctl migrate-layout` to see why")
    return notice(report)


__all__ = [
    "AGENTS", "AUTO_MIGRATE_VARIABLE", "CURRENT", "FIXED_LOCK_STEMS", "LEGACY", "LEGACY_SUPPORT_ENDS",
    "BRIDGES", "LOCKS", "REMOVAL_ISSUE", "RENAMES_TRIPWIRE", "RENAMES_TRIPWIRE_TEXT", "RESERVED_NAMES", "TRIPWIRE", "TRIPWIRE_TEXT", "agents_directory",
    "auto_migrate", "current_directory", "current_names", "directory", "directory_at", "is_agent_name",
    "is_tripwire", "legacy_directory", "legacy_lock_files", "legacy_names", "locate", "lock_paths",
    "lock_still_current", "locks_directory", "migrate", "names", "notice", "prepare", "prepare_locks", "record_directories", "stray_names",
]
