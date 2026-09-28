"""Run a command boxed to one slot: resource limits plus a private file-system view.

``wrkslots run SLOT -- COMMAND`` needs no root and no host configuration:

* **Limits.** Each slot gets its own slice, ``wrkslots-<slot>.slice``, under
  ``wrkslots.slice``. Memory, CPU, task, and IO limits apply to the slice, so
  every command run against the same slot shares one budget. Optional limits on
  ``wrkslots.slice`` bound all slots together (the machine-wide guard).
* **Process tree.** COMMAND runs in a transient systemd *scope* (``systemd-run
  --user --scope``), not a service, so it stays a child of the caller. A
  terminal multiplexer that detects agents by walking a pane's process tree
  (Herdr) still sees the agent, and the caller's terminal is used directly.
* **File-system view.** Inside the scope, a helper enters a new user and mount
  namespace (``unshare(CLONE_NEWUSER|CLONE_NEWNS)``, the current user mapped to
  itself) and builds the view before exec'ing COMMAND. By default
  (``protect_system``) every mount becomes read-only
  except the slot itself, its private state, the Git common directories its
  checkouts commit into, and the wrkslots control directory (for heartbeats).
  ``/tmp`` and the chosen ``$HOME`` state directories are redirected into the
  slot's private state, so they are bounded by the slot's image when it has one.
* **HOME policy.** ``ro`` exposes all of ``$HOME`` read-only (tools in ``~/bin``
  keep working); ``select`` exposes only the listed ``$HOME``-relative paths;
  ``none`` exposes nothing from ``$HOME``. In every mode the ``home_writable``
  directories (``.cache``, ``.buck`` ...) are private, writable, per-slot copies,
  and the ``home_shared`` paths (agent credentials and transcripts such as
  ``.claude`` and ``.codex``) stay shared with the host and writable.

The network is not touched: agent harnesses and site policy own that.
"""

from __future__ import annotations

import ctypes
import dataclasses
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Mapping, Sequence

HOME_MODES = ("ro", "select", "none")
ISOLATIONS = ("namespace", "cgroup")
DEFAULT_HOME_WRITABLE = (".cache", ".buck", ".local/state")
#: Agent harness state that must stay shared with the host and writable:
#: credentials, settings, and session transcripts. A private copy per slot
#: would log the agent out and scatter its history. Top-level files that tools
#: rewrite atomically (~/.claude.json) cannot be bind mounts; in "ro" mode they
#: are seeded as private copies instead.
DEFAULT_HOME_SHARED = (
    ".claude",
    ".codex",
    ".muse",
    ".config/muse",
    ".config/opencode",
    ".local/share/opencode",
    ".local/state/herdr",
)
DEFAULT_TASKS_MAX = 8192
_ENV_DENYLIST = frozenset(
    {
        "INVOCATION_ID",
        "JOURNAL_STREAM",
        "NOTIFY_SOCKET",
        "LISTEN_FDS",
        "LISTEN_PID",
        "LISTEN_FDNAMES",
        "MANAGERPID",
        "SYSTEMD_EXEC_PID",
        "TMPDIR",
        "TMP",
        "TEMP",
    }
)
_LIMIT_RE = re.compile(r"^(infinity|\d+(\.\d+)?[KMGT]?|\d+(\.\d+)?%)$")


class SandboxError(RuntimeError):
    """A sandbox request is invalid or cannot run on this host."""


@dataclasses.dataclass(frozen=True)
class SandboxSettings:
    """Defaults for ``wrkslots run``; every field can be overridden per run."""

    home: str = "ro"
    home_paths: tuple[str, ...] = ()
    home_writable: tuple[str, ...] = DEFAULT_HOME_WRITABLE
    home_shared: tuple[str, ...] = DEFAULT_HOME_SHARED
    read_write: tuple[str, ...] = ()
    protect_system: bool = True
    memory_max: str | None = None
    memory_high: str | None = None
    cpu_quota: str | None = None
    tasks_max: int | None = DEFAULT_TASKS_MAX
    io_weight: int | None = None
    all_slots_memory_max: str | None = None
    all_slots_cpu_quota: str | None = None


def _str_tuple(value: object, label: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise SandboxError(f"{label} must be a list of strings")
    return tuple(str(item) for item in value)


def _optional_str(value: object, label: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or not _LIMIT_RE.match(value):
        raise SandboxError(f"{label} must look like 32G, 800%, or infinity, not {value!r}")
    return value


def _optional_int(value: object, label: str) -> int | None:
    if value is None:
        return None
    if not isinstance(value, int) or isinstance(value, bool) or value < 1:
        raise SandboxError(f"{label} must be a positive integer")
    return value


def _home_relative(value: str, label: str) -> str:
    candidate = Path(value)
    if not value or candidate.is_absolute() or ".." in candidate.parts:
        raise SandboxError(f"{label} entries must be $HOME-relative paths without '..': {value!r}")
    return candidate.as_posix()


def settings_from_obj(raw: Mapping[str, object]) -> SandboxSettings:
    """Validate a configuration.sandbox mapping into settings."""

    known = {field.name for field in dataclasses.fields(SandboxSettings)}
    unknown = sorted(set(raw) - known)
    if unknown:
        raise SandboxError(f"configuration.sandbox has unknown keys: {', '.join(unknown)}")
    defaults = SandboxSettings()
    home = raw.get("home", defaults.home)
    if home not in HOME_MODES:
        raise SandboxError(f"configuration.sandbox.home must be one of {', '.join(HOME_MODES)}")
    protect = raw.get("protect_system", defaults.protect_system)
    if not isinstance(protect, bool):
        raise SandboxError("configuration.sandbox.protect_system must be true or false")
    return SandboxSettings(
        home=str(home),
        home_paths=tuple(
            _home_relative(item, "home_paths")
            for item in _str_tuple(raw.get("home_paths", []), "configuration.sandbox.home_paths")
        ),
        home_writable=tuple(
            _home_relative(item, "home_writable")
            for item in _str_tuple(
                raw.get("home_writable", list(defaults.home_writable)),
                "configuration.sandbox.home_writable",
            )
        ),
        home_shared=tuple(
            _home_relative(item, "home_shared")
            for item in _str_tuple(
                raw.get("home_shared", list(defaults.home_shared)),
                "configuration.sandbox.home_shared",
            )
        ),
        read_write=_str_tuple(raw.get("read_write", []), "configuration.sandbox.read_write"),
        protect_system=protect,
        memory_max=_optional_str(raw.get("memory_max"), "memory_max"),
        memory_high=_optional_str(raw.get("memory_high"), "memory_high"),
        cpu_quota=_optional_str(raw.get("cpu_quota"), "cpu_quota"),
        tasks_max=_optional_int(raw.get("tasks_max", defaults.tasks_max), "tasks_max"),
        io_weight=_optional_int(raw.get("io_weight"), "io_weight"),
        all_slots_memory_max=_optional_str(raw.get("all_slots_memory_max"), "all_slots_memory_max"),
        all_slots_cpu_quota=_optional_str(raw.get("all_slots_cpu_quota"), "all_slots_cpu_quota"),
    )


def validate_limit(value: str | None, label: str) -> str | None:
    """Validate one systemd limit value such as 32G or 800%."""

    return _optional_str(value, label)


@dataclasses.dataclass(frozen=True)
class SlotView:
    """Everything about a slot that the sandbox must make visible or writable."""

    slot: str
    slot_type: str
    slot_path: Path
    state_directory: Path
    git_directories: tuple[Path, ...]
    control_directory: Path
    representation: str


def systemd_escape(value: str) -> str:
    """Escape a string for use inside a systemd unit name."""

    result = subprocess.run(
        ["systemd-escape", "--", value], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise SandboxError(f"systemd-escape failed for {value!r}: {result.stderr.strip()}")
    return result.stdout.strip()


def parent_slice(cgroup_file: Path = Path("/proc/self/cgroup")) -> str | None:
    """The innermost user-manager slice enclosing the caller, if any.

    wrkslots never moves a process OUT of an enclosing slice: a caller that site
    policy placed in a sandbox slice (an agent harness's jail, for example) must
    stay inside it, because monitoring and limits are scoped to that cgroup. The
    slot's slices are therefore created as children of the caller's slice.
    """

    try:
        text = cgroup_file.read_text(encoding="utf-8")
    except OSError:
        return None
    for line in text.splitlines():
        if not line.startswith("0::"):
            continue
        parts = line[3:].split("/")
        if "user@" not in line:
            return None
        below = parts[next(i for i, part in enumerate(parts) if part.startswith("user@")) + 1 :]
        slices = [part for part in below if part.endswith(".slice")]
        return slices[-1] if slices else None
    return None


def root_slice(parent: str | None = None) -> str:
    """The slice holding every slot's slice (the all-slots guard)."""

    enclosing = parent if parent is not None else parent_slice()
    prefix = "" if enclosing is None else enclosing.removesuffix(".slice") + "-"
    return f"{prefix}wrkslots.slice"


def slice_name(slot_type: str, slot: str, parent: str | None = None) -> str:
    # A dash in a slice name means nesting, so the slot name is escaped: slot
    # "kvm-foo" becomes one child of the wrkslots slice, not three levels.
    """The slice for one slot, nested under the caller's slice unless ``parent`` is given."""

    label = slot if slot_type == "agent" else f"{slot_type}:{slot}"
    return root_slice(parent).removesuffix(".slice") + f"-{systemd_escape(label)}.slice"


def _mount_rows(mountinfo: Path = Path("/proc/self/mountinfo")) -> list[tuple[str, str, Path]]:
    rows: list[tuple[str, str, Path]] = []
    for line in mountinfo.read_text(encoding="utf-8", errors="surrogateescape").splitlines():
        fields = line.split(" - ", 1)[0].split()
        if len(fields) >= 5:
            rows.append((fields[2], fields[3], Path(fields[4].replace("\\040", " "))))
    return rows


def path_aliases(path: Path) -> list[Path]:
    """Return ``path`` plus other mount paths that expose the same directory.

    On hosts where ``$HOME`` is a bind mount of a subdirectory of a larger file
    system, the same files are also reachable through that file system's own
    mount point. Hiding ``$HOME`` without hiding its alias would hide nothing.
    """

    aliases = [path]
    try:
        rows = _mount_rows()
    except OSError:
        return aliases
    home_rows = [row for row in rows if row[2] == path]
    if not home_rows:
        return aliases
    device, root, _mount_point = home_rows[-1]
    if root == "/":
        return aliases
    for other_device, other_root, other_mount in rows:
        if other_device == device and other_root == "/" and other_mount != path:
            alias = other_mount / root.lstrip("/")
            if alias.is_dir() and alias not in aliases:
                aliases.append(alias)
    return aliases


def forwarded_environment(environ: Mapping[str, str]) -> dict[str, str]:
    """The caller's environment minus service-manager and temp-directory variables."""

    return {
        key: value
        for key, value in environ.items()
        if key not in _ENV_DENYLIST and "=" not in key and "\n" not in value
    }


def prepare_state(view: SlotView, settings: SandboxSettings, home: Path) -> None:
    """Create the private state directories and any missing bind targets."""

    for name in ("home", "tmp", f"home-{settings.home}"):
        (view.state_directory / name).mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(view.state_directory / "tmp", 0o1777)
    for relative in settings.home_writable:
        (view.state_directory / "home" / relative).mkdir(parents=True, exist_ok=True)
        # Bind targets are created inside the sandbox's writable $HOME layer.


def slice_limit_properties(settings: SandboxSettings) -> list[str]:
    """systemd properties for the per-slot slice."""

    limits: list[str] = []
    if settings.memory_max:
        limits.append(f"MemoryMax={settings.memory_max}")
        limits.append("MemorySwapMax=0")
    if settings.memory_high:
        limits.append(f"MemoryHigh={settings.memory_high}")
    if settings.cpu_quota:
        limits.append(f"CPUQuota={settings.cpu_quota}")
    if settings.tasks_max:
        limits.append(f"TasksMax={settings.tasks_max}")
    if settings.io_weight:
        limits.append(f"IOWeight={settings.io_weight}")
    return limits


def apply_slice_limits(view: SlotView, settings: SandboxSettings) -> list[str]:
    """Set runtime limits on the slot's slice (and the all-slots slice)."""

    applied: list[str] = []
    root_limits: list[str] = []
    if settings.all_slots_memory_max:
        root_limits += [f"MemoryMax={settings.all_slots_memory_max}", "MemorySwapMax=0"]
    if settings.all_slots_cpu_quota:
        root_limits.append(f"CPUQuota={settings.all_slots_cpu_quota}")
    for unit, limits in (
        (root_slice(), root_limits),
        (slice_name(view.slot_type, view.slot), slice_limit_properties(settings)),
    ):
        if not limits:
            continue
        # A slice has to be loaded before its properties can be set; starting
        # it is idempotent and leaves no process behind.
        subprocess.run(
            ["systemctl", "--user", "start", unit], capture_output=True, check=False
        )
        result = subprocess.run(
            ["systemctl", "--user", "set-property", "--runtime", unit, *limits],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            raise SandboxError(f"cannot set limits on {unit}: {result.stderr.strip()}")
        applied.append(f"{unit}: {' '.join(limits)}")
    return applied


# ----------------------------------------------------------------- the view spec

#: Mount trees left writable even under protect_system, as systemd's
#: ProtectSystem=strict does: kernel interfaces and per-user runtime sockets.
_ALWAYS_WRITABLE_PREFIXES = ("/proc", "/sys", "/dev", "/run/user")


def build_spec(view: SlotView, settings: SandboxSettings, home: Path, cwd: Path) -> dict[str, object]:
    """Describe the mount namespace the helper must build, as plain JSON."""

    writable: list[str] = [str(view.slot_path), str(view.state_directory), str(view.control_directory)]
    writable.extend(str(path) for path in view.git_directories)
    writable.extend(os.path.abspath(item) for item in settings.read_write)
    writable.extend(str(home / relative) for relative in settings.home_shared)
    hide: list[str] = []
    read_only_binds: list[str] = []
    aliases = path_aliases(home)
    # $HOME inside the sandbox is a private, persistent, per-slot directory.
    # "ro" exposes every top-level entry of the real $HOME in it: directories
    # as read-only bind mounts, small files as private copies (seeded once), so
    # tools that atomically rewrite a top-level dotfile (~/.claude.json) keep
    # working while the host's $HOME is never modified. "select" exposes only
    # the listed paths; "none" exposes nothing.
    home_layer: dict[str, object] = {
        "target": str(home),
        # One private home per exposure mode: files seeded for "ro" must not
        # leak into a later "none" run of the same slot.
        "source": str(view.state_directory / f"home-{settings.home}"),
        "expose": settings.home,
        "paths": list(settings.home_paths),
        "skip": sorted({relative.split("/", 1)[0] for relative in (*settings.home_shared, *settings.home_writable)}),
    }
    hide = [str(alias) for alias in aliases[1:]]
    redirects: list[list[str]] = [[str(view.state_directory / "tmp"), "/tmp"]]
    for relative in settings.home_writable:
        redirects.append([str(view.state_directory / "home" / relative), str(home / relative)])
    return {
        "protect_system": settings.protect_system,
        "home": home_layer,
        "hide": hide,
        "writable": writable,
        "read_only_binds": read_only_binds,
        "redirects": redirects,
        "cwd": str(cwd),
    }


# ------------------------------------------------------------------ the helper

_CLONE_NEWNS = 0x00020000
_CLONE_NEWUSER = 0x10000000
_MS_RDONLY = 1
_MS_NOSUID = 2
_MS_NODEV = 4
_MS_NOEXEC = 8
_MS_REMOUNT = 32
_MS_NOATIME = 1024
_MS_NODIRATIME = 2048
_MS_BIND = 4096
_MS_REC = 16384
_MS_PRIVATE = 1 << 18
_MS_RELATIME = 1 << 21
_MS_STRICTATIME = 1 << 24
_O_PATH = 0o10000000
_OPTION_FLAGS = {
    "nosuid": _MS_NOSUID,
    "nodev": _MS_NODEV,
    "noexec": _MS_NOEXEC,
    "noatime": _MS_NOATIME,
    "nodiratime": _MS_NODIRATIME,
    "relatime": _MS_RELATIME,
    "strictatime": _MS_STRICTATIME,
}


def _libc() -> ctypes.CDLL:
    return ctypes.CDLL(None, use_errno=True)


def _check(result: int, what: str) -> None:
    if result != 0:
        error = ctypes.get_errno()
        raise SandboxError(f"{what}: {os.strerror(error)}")


def _mount(source: str | None, target: str, fstype: str | None, flags: int, data: str | None = None) -> None:
    libc = _libc()
    libc.mount.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_ulong, ctypes.c_char_p]
    _check(
        libc.mount(
            source.encode() if source is not None else None,
            target.encode(),
            fstype.encode() if fstype is not None else None,
            flags,
            data.encode() if data is not None else None,
        ),
        f"mount {source or fstype} on {target}",
    )


def _write(path: str, text: str) -> None:
    with open(path, "w", encoding="ascii") as handle:
        handle.write(text)


def _under(path: str, roots: Sequence[str]) -> bool:
    return any(path == root or path.startswith(root.rstrip("/") + "/") for root in roots)


def _mounts() -> list[tuple[str, int]]:
    """(mount point, flags to preserve) for every mount, parents first."""

    rows: list[tuple[str, int]] = []
    with open("/proc/self/mountinfo", encoding="utf-8", errors="surrogateescape") as handle:
        for line in handle:
            fields = line.split(" - ", 1)[0].split()
            if len(fields) < 6:
                continue
            point = fields[4].replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\")
            flags = 0
            for option in fields[5].split(","):
                flags |= _OPTION_FLAGS.get(option, 0)
            rows.append((point, flags))
    return rows


_SEED_COPY_LIMIT = 1024 * 1024


def _bind_fd(descriptor: int, target: str, *, read_only_bind: bool = False) -> None:
    source_path = f"/proc/self/fd/{descriptor}"
    if not os.path.lexists(target):
        if os.path.isdir(source_path):
            os.makedirs(target, exist_ok=True)
        else:
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "a", encoding="utf-8"):
                pass
    _mount(source_path, target, None, _MS_BIND | _MS_REC)
    if read_only_bind:
        _mount(None, target, None, _MS_BIND | _MS_REMOUNT | _MS_RDONLY)


def _build_home(home_spec: Mapping[str, object], pinned: dict[str, int]) -> str:
    """Cover $HOME with the slot's private home and expose what the mode allows."""

    import shutil as _shutil
    import stat as _stat

    target = str(home_spec["target"])
    source = str(home_spec["source"])
    expose = str(home_spec.get("expose", "ro"))
    raw_paths = home_spec.get("paths", [])
    raw_skip = home_spec.get("skip", [])
    paths = [str(item) for item in raw_paths] if isinstance(raw_paths, list) else []
    skip = {str(item) for item in raw_skip} if isinstance(raw_skip, list) else set()
    real = os.open(target, _O_PATH | os.O_DIRECTORY)
    pinned.setdefault(source, os.open(source, _O_PATH))
    if expose == "ro":
        names = sorted(set(os.listdir(f"/proc/self/fd/{real}")) - skip)
    elif expose == "select":
        names = sorted(set(paths))
    else:
        names = []
    entries: list[tuple[str, int, os.stat_result, str]] = []
    for name in names:
        try:
            info = os.stat(name, dir_fd=real, follow_symlinks=False)
            link = os.readlink(name, dir_fd=real) if _stat.S_ISLNK(info.st_mode) else ""
            descriptor = os.open(name, _O_PATH | os.O_NOFOLLOW, dir_fd=real)
        except OSError:
            continue
        entries.append((name, descriptor, info, link))
    _mount(f"/proc/self/fd/{pinned[source]}", target, None, _MS_BIND)
    for name, descriptor, info, link in entries:
        destination = os.path.join(target, name)
        source_path = f"/proc/self/fd/{descriptor}"
        if _stat.S_ISLNK(info.st_mode):
            if not os.path.lexists(destination):
                os.makedirs(os.path.dirname(destination), exist_ok=True)
                os.symlink(link, destination)
        elif _stat.S_ISREG(info.st_mode) and info.st_size <= _SEED_COPY_LIMIT:
            if not os.path.lexists(destination):
                os.makedirs(os.path.dirname(destination), exist_ok=True)
                _shutil.copy2(source_path, destination)
        else:
            _bind_fd(descriptor, destination, read_only_bind=True)
        os.close(descriptor)
    os.close(real)
    return target


def enter(spec: Mapping[str, object], command: Sequence[str]) -> None:
    """Build the view described by ``spec`` in new namespaces, then exec ``command``."""

    uid, gid = os.getuid(), os.getgid()
    libc = _libc()
    _check(libc.unshare(_CLONE_NEWUSER | _CLONE_NEWNS), "unshare user+mount namespace")
    _write("/proc/self/setgroups", "deny")
    _write("/proc/self/uid_map", f"{uid} {uid} 1\n")
    _write("/proc/self/gid_map", f"{gid} {gid} 1\n")
    _mount(None, "/", None, _MS_REC | _MS_PRIVATE)

    def strings(key: str) -> list[str]:
        value = spec.get(key, [])
        assert isinstance(value, list)
        return [str(item) for item in value]

    writable = [path for path in strings("writable") if os.path.exists(path)]
    read_only = [path for path in strings("read_only_binds") if os.path.exists(path)]
    raw_redirects = spec.get("redirects", [])
    assert isinstance(raw_redirects, list)
    redirects = [
        (str(pair[0]), str(pair[1]))
        for pair in raw_redirects
        if isinstance(pair, list) and len(pair) == 2 and os.path.isdir(str(pair[0]))
    ]
    # Pin every source before anything is hidden: once a tmpfs covers $HOME,
    # paths below it are only reachable through these descriptors.
    pinned: dict[str, int] = {}
    for path in [*writable, *read_only, *(source for source, _target in redirects)]:
        if path not in pinned:
            pinned[path] = os.open(path, _O_PATH)

    home_target = ""
    raw_home = spec.get("home")
    if isinstance(raw_home, dict):
        home_target = _build_home({str(key): value for key, value in raw_home.items()}, pinned)

    for hidden in strings("hide"):
        if os.path.isdir(hidden):
            _mount("tmpfs", hidden, "tmpfs", _MS_NOSUID | _MS_NODEV, "mode=0755,size=16m")

    def bind(source: str, target: str, *, read_only_bind: bool = False) -> None:
        _bind_fd(pinned[source], target, read_only_bind=read_only_bind)

    # Redirects first (/tmp, private $HOME state), so that a writable path that
    # happens to live below a redirected directory is bound back on top of it.
    for source, target in redirects:
        bind(source, target)
    for path in read_only:
        bind(path, path, read_only_bind=True)
    for path in writable:
        bind(path, path)

    if spec.get("protect_system", True):
        keep = [*writable, *(target for _source, target in redirects), *_ALWAYS_WRITABLE_PREFIXES]
        failures: list[str] = []
        for point, flags in _mounts():
            if _under(point, keep) or point == home_target:
                continue
            if not os.path.lexists(point):
                continue  # covered by a hidden-path tmpfs: unreachable, nothing to protect
            try:
                _mount(None, point, None, _MS_BIND | _MS_REMOUNT | _MS_RDONLY | flags)
            except SandboxError as exc:
                # EINVAL: the path no longer names that mount's root because a
                # layer above (the private $HOME) shadows it; unreachable.
                # Other failures (another user's FUSE mount, for example) are
                # harmless when this user could not write there anyway.
                if "Invalid argument" in str(exc):
                    continue
                if os.access(point, os.W_OK):
                    failures.append(str(exc))
        if failures:
            raise SandboxError(
                "could not make every mount read-only (refusing to run unprotected): "
                + "; ".join(failures[:3])
            )
    for descriptor in pinned.values():
        os.close(descriptor)
    cwd = str(spec.get("cwd", "/"))
    os.chdir(cwd)
    os.execvp(command[0], list(command))


def _main(argv: Sequence[str] | None = None) -> int:
    """``python -m wrkslots.sandbox enter SPEC-JSON -- COMMAND...`` (internal)."""

    import json

    values = list(sys.argv[1:] if argv is None else argv)
    if len(values) < 4 or values[0] != "enter" or values[2] != "--":
        print("usage: python -m wrkslots.sandbox enter SPEC-JSON -- COMMAND...", file=sys.stderr)
        return 2
    try:
        loaded: object = json.loads(values[1])
        if not isinstance(loaded, dict):
            raise SandboxError("sandbox spec must be a JSON object")
        enter({str(key): value for key, value in loaded.items()}, values[3:])
    except (SandboxError, OSError) as exc:
        print(f"wrkslots sandbox: {exc}", file=sys.stderr)
        return 126
    return 127


# --------------------------------------------------------------- the launcher


def helper_command(spec: Mapping[str, object], command: Sequence[str]) -> list[str]:
    """Argv that re-enters this module to build the namespace view, then runs ``command``."""

    import json

    if not command:
        raise SandboxError("no command to run")
    package_parent = str(Path(__file__).resolve().parent.parent)
    return [
        "env",
        f"PYTHONPATH={package_parent}",
        sys.executable,
        "-m",
        "wrkslots.sandbox",
        "enter",
        json.dumps(dict(spec), sort_keys=True),
        "--",
        *command,
    ]


def place_in_scope(unit: str, slice_unit: str) -> bool:
    """Move THIS process into a new transient scope without forking.

    Keeping the PID matters: a terminal multiplexer that checks whether a pane's
    own shell is at its prompt (Herdr) must still see the same process after
    ``exec wrkslots run ...`` replaces the pane's shell. Returns False when the
    user manager's D-Bus API is unavailable, so the caller can fall back to
    ``systemd-run --scope`` (which forks).
    """

    if shutil.which("busctl") is None:
        return False
    result = subprocess.run(
        [
            "busctl",
            "--user",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            unit,
            "fail",
            "2",
            "PIDs",
            "au",
            "1",
            str(os.getpid()),
            "Slice",
            "s",
            slice_unit,
            "0",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return False
    for _ in range(100):
        try:
            if f"/{unit}" in Path("/proc/self/cgroup").read_text(encoding="utf-8"):
                return True
        except OSError:
            break
        time.sleep(0.01)
    return False


def run(
    view: SlotView,
    settings: SandboxSettings,
    command: Sequence[str],
    *,
    cwd: Path | None = None,
    print_only: bool = False,
    unit: str | None = None,
    isolation: str = "namespace",
) -> int:
    """Replace this process with ``command`` boxed to the slot; never returns.

    ``isolation="namespace"`` (default) applies limits AND the private
    file-system view. ``isolation="cgroup"`` applies limits only: use it for
    programs that need setuid helpers (sudo, or a harness launcher that enters
    its own site sandbox through one), which cannot work inside a user
    namespace; such harnesses bring their own file-system jail.
    """

    if isolation not in ISOLATIONS:
        raise SandboxError(f"isolation must be one of {', '.join(ISOLATIONS)}")
    if shutil.which("systemd-run") is None:
        raise SandboxError("systemd-run is not available; the sandbox needs a systemd user manager")
    home = Path(os.environ.get("HOME", str(Path.home())))
    unit_name = unit or f"wrkslots-run-{systemd_escape(view.slot)}-{os.getpid()}.scope"
    slot_slice = slice_name(view.slot_type, view.slot)
    spec = build_spec(view, settings, home, cwd or view.slot_path)
    helper = helper_command(spec, command) if isolation == "namespace" else list(command)
    if print_only:
        print(f"# scope {unit_name} in {slot_slice}")
        for line in slice_limit_properties(settings):
            print(f"# slice {slot_slice}: {line}")
        print(" \\\n  ".join(_quote(part) for part in helper))
        return 0
    if isolation == "namespace":
        prepare_state(view, settings, home)
    for line in apply_slice_limits(view, settings):
        print(f"wrkslots run: limits {line}", file=sys.stderr)
    environment = forwarded_environment(os.environ)
    environment.update(
        {
            "TMPDIR": "/tmp",
            "WRKSLOTS_SANDBOX": "1",
            "WRKSLOTS_SLOT": view.slot,
            "WRKSLOTS_SLOT_TYPE": view.slot_type,
            "WRKSLOTS_SLOT_PATH": str(view.slot_path),
            "WRKSLOTS_SLOT_REPRESENTATION": view.representation,
        }
    )
    if isolation == "cgroup":
        environment.pop("TMPDIR", None)
        os.chdir(cwd or view.slot_path)
    if place_in_scope(unit_name, slot_slice):
        os.execvpe(helper[0], helper, environment)
    argv = [
        "systemd-run",
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        f"--unit={unit_name}",
        f"--slice={slot_slice}",
        "--",
        *helper,
    ]
    os.execvpe(argv[0], argv, environment)


def _quote(value: str) -> str:
    import shlex

    return shlex.quote(value)


if __name__ == "__main__":
    raise SystemExit(_main())
