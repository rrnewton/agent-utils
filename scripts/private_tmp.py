"""One private temp root per run, outside the host temp dir (`#2383 private-run-tmpdir`).

Validation steps inherit the caller's `TMPDIR`, which is usually unset, so before this every
test and gate-script temp site wrote to the host's `/tmp`. A step that dagrun stops at its
timeout is killed with `cgroup.kill` (SIGKILL), so in-test cleanup cannot be relied on, and
leftovers piled up there (#2382).

`activate()` gives a run one private directory and points `TMPDIR`, `TMP` and `TEMP` at it
(Python reads TMPDIR, TEMP, TMP; Node TMPDIR, TMP, TEMP; Rust's `std::env::temp_dir` TMPDIR),
for this process and every child. The root is removed when the run ends, passed or failed;
validate and gate.py turn SIGTERM into an ordinary exit, so that path removes it too. A root
that a SIGKILL left behind is removed by a later run once it is this user's, untouched for
`STALE_SECONDS`, and its PID is gone (the #941 rules).

Where the root lives:

- Under `$AGENT_UTILS_TMP_BASE`, default `~/.aut`. It is on disk, not tmpfs (tmpfs pages count
  against step memory caps, #447), outside any Git working tree, and not in a synced path.
- SHORT: a Unix socket path is limited to 108 bytes and tests bind sockets under `TMPDIR`. The
  agentctl headless control socket alone is `TMPDIR` plus about 63 bytes, so a root longer than
  about 44 characters breaks those tests (#447, #869). A root is named by its run's PID alone
  (`~/.aut/1234567`, 25 characters here), with one letter added when a dead run's root still
  holds that name, and a root longer than `LONG_ROOT_WARNING` characters is reported.

A root an outer run already made (gate.py around validate, `with-private-tmp` around anything)
is passed down in `AGENT_UTILS_RUN_TMP` and reused, never nested; only the run that made it
removes it. `AGENT_UTILS_PRIVATE_TMP=0` turns the whole mechanism off for one run.

Removal never goes through a mount point: when `/proc/self/mountinfo` lists any mount at or
under the root, the root is kept and the mounts are reported (a test that leaked a bind mount
must not have its source deleted through it).

The one scoped exception (interim, until #2415 wrkslots-tests-temp-independent): the wrkslots
test steps listed in `HOST_TEMP_STEPS` run under `with-private-tmp --host-temp`, which gives
them a fresh root at `/tmp/aut-<uid>/<n>` instead. Their tests model wrkslots' ownerless
validation exclusion, which needs a root-owned sticky parent on the same filesystem as the
fenced path, and the fixtures name `/tmp` itself as that parent. They also walk Git markers up
from `tmp_path` inside a user namespace, where an automounted home parent refuses the lookup. Neither
holds for a root under the home directory on every host. Such a root:

- has an owner record `<n>.owner` beside it (pid, process start time, PID namespace), written
  and locked before the root exists and held locked for the run. `sweep_host_temp` removes a
  root only when that lock is free and the recorded process is gone, never by name alone, and
  logs the bytes it removed. It runs at every host-temp activation, at gate start, and from the
  daily tick (`python3 scripts/private_tmp.py sweep`);
- is bounded: the wrapper measures it every `HOST_TEMP_POLL_SECONDS` and stops the command
  with a named refusal (exit `OVER_LIMIT_EXIT`) once it passes `HOST_TEMP_LIMIT_BYTES`, so a
  step can never fill the host temp dir;
- is allowed only for those steps: a validation-graph contract test fails the gate when any
  other step asks for it.
"""

from __future__ import annotations

import contextlib
import dataclasses
import fcntl
import json
import os
import re
import shutil
import stat
import string
import sys
import tempfile
import time
from collections.abc import Iterator, Mapping
from pathlib import Path

BASE_ENV = "AGENT_UTILS_TMP_BASE"
HOST_BASE_ENV = "AGENT_UTILS_HOST_TMP_BASE"
MARKER_ENV = "AGENT_UTILS_RUN_TMP"
SWITCH_ENV = "AGENT_UTILS_PRIVATE_TMP"
TEMP_VARIABLES = ("TMPDIR", "TMP", "TEMP")
STALE_SECONDS = 24 * 3600
LONG_ROOT_WARNING = 30
"""Characters; above this, socket-binding tests (agentctl headless needs <= about 44) lose their margin."""
MOUNTINFO = Path("/proc/self/mountinfo")
_ROOT_NAME = re.compile(r"^(\d+)[a-z]?$")

#: The scoped exception's parent (see the module docstring). Literally `/tmp`, not
#: `tempfile.gettempdir()`: the wrkslots fixtures name `/tmp` as their trusted sticky parent, so
#: the roots must sit on its filesystem whatever TMPDIR says.
HOST_TEMP = Path("/tmp")
#: The only steps that may run under `with-private-tmp --host-temp` (fnmatch patterns over step
#: tags). `python.wrkslots.host-*` is admitted ahead of #2065's host-visible wrkslots shards.
#: Interim: #2415 makes these tests independent of the temp location and removes this list.
HOST_TEMP_STEPS = (
    "python-lifecycle.*",
    "python.wrkslots.shard-*",
    "python.wrkslots.host-*",
    "rust.wrkslots.test",
    "repository.check.wrkslots-debug-test",
)
HOST_TEMP_LIMIT_BYTES = 5_000_000_000
"""Disk bytes a host-temp root may hold before its command is stopped (#2383, liaison 1081)."""
HOST_TEMP_POLL_SECONDS = 10.0
STOP_GRACE_SECONDS = 30.0
"""After the size refusal's SIGTERM, how long the command gets before SIGKILL."""
OVER_LIMIT_EXIT = 73
"""The wrapper's exit status when it refuses: no usable host-temp root, or a root over its bound."""
OWNER_SUFFIX = ".owner"


class HostTempUnavailable(OSError):
    """No host-temp root could be made (`activate(host_temp=True)`); the wrkslots steps that
    need one cannot run anywhere else, so their wrapper refuses."""


class MountUnderRoot(RuntimeError):
    """A root still has a mount at or under it, so it was kept."""

    def __init__(self, root: Path, mounts: list[Path]) -> None:
        self.root = root
        self.mounts = mounts
        shown = ", ".join(str(mount) for mount in mounts)
        super().__init__(f"kept {root}: mount(s) under it: {shown}")


def enabled(environ: Mapping[str, str] = os.environ) -> bool:
    """False only when `AGENT_UTILS_PRIVATE_TMP=0` opts this run out."""
    return environ.get(SWITCH_ENV, "1").strip() != "0"


def base_dir(environ: Mapping[str, str] = os.environ) -> Path:
    """Where roots are made: `$AGENT_UTILS_TMP_BASE`, else `~/.aut`."""
    configured = environ.get(BASE_ENV, "").strip()
    return Path(configured).expanduser() if configured else Path.home() / ".aut"


def inherited_root(environ: Mapping[str, str] = os.environ) -> Path | None:
    """The root an outer run passed down, if it is still a real directory of this user's."""
    value = environ.get(MARKER_ENV, "").strip()
    if not value:
        return None
    path = Path(value)
    try:
        info = path.lstat()
    except OSError:
        return None
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        return None
    return path


def _unescape_mountinfo(field: str) -> str:
    """Undo mountinfo's octal escapes (`\\040` for a space and so on)."""
    return re.sub(r"\\([0-7]{3})", lambda match: chr(int(match.group(1), 8)), field)


def mounts_under(root: Path, mountinfo: Path = MOUNTINFO) -> list[Path]:
    """Every mount point at or under `root` (by the given path and by its resolved path)."""
    try:
        lines = mountinfo.read_text(errors="replace").splitlines()
    except OSError:
        return []
    prefixes = {str(root).rstrip("/")}
    with contextlib.suppress(OSError):
        prefixes.add(str(root.resolve()).rstrip("/"))
    found: list[Path] = []
    for line in lines:
        fields = line.split()
        if len(fields) < 5:
            continue
        point = _unescape_mountinfo(fields[4])
        if any(point == prefix or point.startswith(prefix + "/") for prefix in prefixes):
            found.append(Path(point))
    return found


def _restore_owner_access(path: Path) -> None:
    """Make a test-written tree removable without following its symlinks."""
    if path.is_symlink() or not path.exists():
        return
    path.chmod(path.stat().st_mode | stat.S_IRWXU)
    if path.is_dir():
        for child in path.iterdir():
            _restore_owner_access(child)


def remove_root(root: Path, mountinfo: Path = MOUNTINFO) -> None:
    """Remove a root, including parts a test made read-only; refuse (MountUnderRoot) when any
    mount is at or under it, without removing anything."""
    if not root.exists():
        return
    mounts = mounts_under(root, mountinfo)
    if mounts:
        raise MountUnderRoot(root, mounts)
    try:
        shutil.rmtree(root)
    except OSError:
        _restore_owner_access(root)
        shutil.rmtree(root)


def sweep_stale(base: Path, *, now: float | None = None, mountinfo: Path = MOUNTINFO) -> list[Path]:
    """Remove roots under `base` that killed runs left behind; returns what was removed.

    A root is stale once it is this user's, has stood untouched for `STALE_SECONDS`, and the
    process its name records is gone. A live run's root is never touched, whatever its age,
    and a root with a mount under it is kept and reported.
    """
    moment = time.time() if now is None else now
    removed: list[Path] = []
    try:
        entries = sorted(base.iterdir())
    except OSError:
        return removed
    for path in entries:
        match = _ROOT_NAME.match(path.name)
        if match is None:
            continue
        try:
            info = path.lstat()
        except OSError:
            continue
        if (
            not stat.S_ISDIR(info.st_mode)
            or info.st_uid != os.getuid()
            or moment - info.st_mtime < STALE_SECONDS
            or Path("/proc", match.group(1)).exists()
        ):
            continue
        try:
            remove_root(path, mountinfo)
        except (MountUnderRoot, OSError) as error:
            print(f"private tmp: could not remove stale root {path}: {error}", file=sys.stderr)
            continue
        removed.append(path)
    return removed


def make_root(base: Path) -> Path:
    """A fresh root `<base>/<pid>` (or `<pid><letter>` when a dead run's root holds that name),
    mode 0700; the base is made 0700 when missing."""
    base.mkdir(mode=0o700, parents=True, exist_ok=True)
    pid = os.getpid()
    for suffix in ("", *string.ascii_lowercase):
        root = base / f"{pid}{suffix}"
        try:
            root.mkdir(mode=0o700)
        except FileExistsError:
            continue
        if len(str(root)) > LONG_ROOT_WARNING:
            print(
                f"private tmp: warning: root {root} is {len(str(root))} characters; tests that "
                f"bind Unix sockets under TMPDIR need it short (agentctl headless: <= about 44). "
                f"Set {BASE_ENV} to a shorter directory on disk.",
                file=sys.stderr,
            )
        return root
    raise FileExistsError(f"private tmp: no free root name under {base}")


def tree_bytes(root: Path) -> int:
    """Disk bytes allocated under `root` (sparse files count what they use), without following
    symlinks or descending into another filesystem; 0 when it is gone."""
    try:
        top = root.lstat()
    except OSError:
        return 0
    total = top.st_blocks * 512
    pending = [root] if stat.S_ISDIR(top.st_mode) else []
    while pending:
        try:
            entries = list(os.scandir(pending.pop()))
        except OSError:
            continue
        for entry in entries:
            try:
                info = entry.stat(follow_symlinks=False)
            except OSError:
                continue
            total += info.st_blocks * 512
            if stat.S_ISDIR(info.st_mode) and info.st_dev == top.st_dev:
                pending.append(Path(entry.path))
    return total


def host_temp_base(environ: Mapping[str, str] = os.environ) -> Path:
    """This user's base for host-temp roots: `/tmp/aut-<uid>`, or `$AGENT_UTILS_HOST_TMP_BASE`
    (for tests, which must not touch the real one; no validation step may set it)."""
    configured = environ.get(HOST_BASE_ENV, "").strip()
    return Path(configured) if configured else HOST_TEMP / f"aut-{os.getuid()}"


def _private_base(base: Path) -> None:
    """Make `base` (mode 0700) or accept it only as a real directory of this user's; raise
    OSError for anything else. In a shared sticky directory nobody else can rename or remove a
    directory of ours once it is checked, so the check holds for the run."""
    with contextlib.suppress(FileExistsError):
        base.mkdir(mode=0o700)
    info = base.lstat()
    if not stat.S_ISDIR(info.st_mode):
        raise OSError(f"{base} is not a real directory (a symlink or a file); refusing to use it")
    if info.st_uid != os.getuid():
        raise OSError(f"{base} belongs to uid {info.st_uid}, not {os.getuid()}; refusing to use it")
    if stat.S_IMODE(info.st_mode) != 0o700:
        os.chmod(base, 0o700)


def process_start(pid: int) -> int | None:
    """A process's start time in clock ticks since boot (/proc/<pid>/stat field 22), or None
    when no such process is visible."""
    try:
        text = Path(f"/proc/{pid}/stat").read_text(errors="replace")
    except OSError:
        return None
    try:
        return int(text[text.rindex(")") + 2:].split()[19])
    except (ValueError, IndexError):
        return None


def _pid_namespace() -> str:
    try:
        return os.readlink("/proc/self/ns/pid")
    except OSError:
        return ""


def make_host_temp_root(base: Path) -> tuple[Path, int]:
    """A fresh root `<base>/<n>` (mode 0700) and the descriptor that holds its owner record
    locked. The record is created, locked and written BEFORE the root exists, so a sweep never
    sees a root of a live run without a held record. Stale roots are swept first."""
    _private_base(base)
    sweep_host_temp(base)
    pid = os.getpid()
    owner = {"pid": pid, "start": process_start(pid), "pid_namespace": _pid_namespace()}
    for suffix in ("", *string.ascii_lowercase):
        name = f"{pid}{suffix}"
        record = base / f"{name}{OWNER_SUFFIX}"
        try:
            descriptor = os.open(
                record, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600
            )
        except FileExistsError:
            continue
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        os.write(descriptor, json.dumps(owner).encode())
        root = base / name
        try:
            root.mkdir(mode=0o700)
        except FileExistsError:
            # A root with no record holds the name: leave it (the sweep reports it).
            os.unlink(record)
            os.close(descriptor)
            continue
        except OSError:
            os.unlink(record)
            os.close(descriptor)
            raise
        return root, descriptor
    raise FileExistsError(f"private tmp: no free root name under {base}")


def release_host_temp_root(root: Path, descriptor: int, *, keep: bool = False) -> None:
    """End a host-temp run: remove its root (unless `keep`, or a mount is under it: both are
    reported), then its owner record, then drop the lock. A kept root stays until a later sweep."""
    removed = False
    if keep:
        print(f"private tmp: kept {root} after a failed run (removed by the next sweep)", file=sys.stderr)
    else:
        try:
            remove_root(root)
            removed = True
        except (MountUnderRoot, OSError) as error:
            print(f"private tmp: {error}", file=sys.stderr)
    if removed:
        with contextlib.suppress(OSError):
            root.with_name(root.name + OWNER_SUFFIX).unlink()
    os.close(descriptor)


def sweep_host_temp(base: Path | None = None, *, mountinfo: Path = MOUNTINFO) -> list[tuple[Path, int]]:
    """Remove host-temp roots whose run is over; return (root, bytes) for each one removed.

    A root goes only when its owner record's lock is free AND the recorded process is gone:
    no process with that pid, or one with a different start time (a reused pid). A record from
    another PID namespace is judged by its lock alone, which no namespace can fake. A record
    that cannot be read, and a root with no record at all, are reported and kept. Never by name
    alone. Each removal and the total are logged with their bytes.
    """
    base = host_temp_base() if base is None else base
    try:
        info = base.lstat()
    except OSError:
        return []
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        print(f"private tmp: {base} is not this user's directory; not sweeping it", file=sys.stderr)
        return []
    removed: list[tuple[Path, int]] = []
    namespace = _pid_namespace()
    try:
        entries = sorted(base.iterdir())
    except OSError:
        return removed
    names = {path.name for path in entries}
    for record in entries:
        if not record.name.endswith(OWNER_SUFFIX):
            continue
        name = record.name[: -len(OWNER_SUFFIX)]
        if _ROOT_NAME.match(name) is None:
            continue
        try:
            descriptor = os.open(record, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        except OSError:
            continue
        try:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                continue  # its run holds the lock: live
            try:
                owner = json.loads(os.read(descriptor, 4096).decode())
                pid, start = int(owner["pid"]), owner["start"]
                owner_namespace = str(owner.get("pid_namespace", ""))
            except (OSError, ValueError, KeyError, TypeError):
                print(f"private tmp: unreadable owner record {record}; kept", file=sys.stderr)
                continue
            if owner_namespace == namespace and start is not None and process_start(pid) == start:
                print(f"private tmp: {record} is unlocked but pid {pid} still runs; kept", file=sys.stderr)
                continue
            root = base / name
            size = tree_bytes(root)
            try:
                remove_root(root, mountinfo)
            except (MountUnderRoot, OSError) as error:
                print(f"private tmp: could not sweep {root}: {error}", file=sys.stderr)
                continue
            with contextlib.suppress(OSError):
                record.unlink()
            print(f"private tmp: swept {root} ({size} bytes; its run, pid {pid}, is over)", file=sys.stderr)
            removed.append((root, size))
        finally:
            os.close(descriptor)
    for path in entries:
        if _ROOT_NAME.match(path.name) and f"{path.name}{OWNER_SUFFIX}" not in names and path.is_dir():
            print(f"private tmp: {path} has no owner record; kept (remove it by hand)", file=sys.stderr)
    if removed:
        total = sum(size for _, size in removed)
        print(f"private tmp: swept {len(removed)} root(s) under {base}, {total} bytes", file=sys.stderr)
    return removed


def environment(root: Path) -> dict[str, str]:
    """The variables that point a child at `root`."""
    return {**{name: str(root) for name in TEMP_VARIABLES}, MARKER_ENV: str(root)}


@dataclasses.dataclass
class Run:
    """One activation: its root (None when the run opted out), whether this run made it, and
    whether it failed. `failed` starts True so an exception counts as a failure; a caller that
    finishes normally sets it from its own verdict."""

    root: Path | None
    owned: bool
    failed: bool = True


@contextlib.contextmanager
def activate(*, keep_on_failure: bool = False, host_temp: bool = False) -> Iterator[Run]:
    """Point this process (and every child it starts) at a private root for the block.

    An inherited root is reused and left for its maker to remove; a root made here is removed
    when the block ends, unless the run failed and `keep_on_failure` is set (then it is kept and
    named). The previous values of the variables, and `tempfile`'s cached directory, are restored.

    `host_temp` (only `with-private-tmp --host-temp`, only for `HOST_TEMP_STEPS`) always makes a
    fresh root under `host_temp_base()` with a locked owner record, even inside an outer run;
    it raises HostTempUnavailable when no such root can be made, because those steps cannot
    run elsewhere.
    """
    if not enabled():
        yield Run(root=None, owned=False)
        return
    lock: int | None = None
    root: Path | None
    if host_temp:
        try:
            root, lock = make_host_temp_root(host_temp_base())
        except OSError as error:
            raise HostTempUnavailable(f"no host-temp root under {host_temp_base()}: {error}") from error
        owned = True
    else:
        root = inherited_root()
        owned = root is None
    if root is None:
        base = base_dir()
        sweep_stale(base)
        try:
            root = make_root(base)
        except OSError as error:
            # Never fail a run over its scratch location; say loudly that it is not private.
            print(f"private tmp: warning: no private root under {base} ({error}); "
                  "this run uses the inherited TMPDIR", file=sys.stderr)
            yield Run(root=None, owned=False)
            return
    run = Run(root=root, owned=owned)
    saved = {name: os.environ.get(name) for name in (*TEMP_VARIABLES, MARKER_ENV)}
    saved_tempdir = tempfile.tempdir
    os.environ.update(environment(root))
    tempfile.tempdir = None
    try:
        yield run
    finally:
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
        tempfile.tempdir = saved_tempdir
        if lock is not None:
            release_host_temp_root(root, lock, keep=run.failed and keep_on_failure)
        elif owned:
            if run.failed and keep_on_failure:
                print(f"private tmp: kept {root} after a failed run", file=sys.stderr)
            else:
                try:
                    remove_root(root)
                except (MountUnderRoot, OSError) as error:
                    print(f"private tmp: {error}", file=sys.stderr)


def main(argv: list[str]) -> int:
    """`python3 scripts/private_tmp.py sweep`: the daily tick's sweep of both bases (host-temp
    roots by their owner records, private roots by the 24-hour rule); prints one line per root
    removed and a total line, and exits 0."""
    if argv != ["sweep"]:
        print("usage: python3 scripts/private_tmp.py sweep", file=sys.stderr)
        return 2
    host = sweep_host_temp()
    home_base = base_dir()
    private = [(path, 0) for path in sweep_stale(home_base)]
    for path, size in host:
        print(f"swept {path} ({size} bytes)")
    for path, _ in private:
        print(f"swept {path}")
    print(
        f"private tmp sweep: {len(host)} host-temp root(s), {sum(size for _, size in host)} bytes, "
        f"under {host_temp_base()}; {len(private)} stale root(s) under {home_base}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
