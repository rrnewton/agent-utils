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
"""

from __future__ import annotations

import contextlib
import dataclasses
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
MARKER_ENV = "AGENT_UTILS_RUN_TMP"
SWITCH_ENV = "AGENT_UTILS_PRIVATE_TMP"
TEMP_VARIABLES = ("TMPDIR", "TMP", "TEMP")
STALE_SECONDS = 24 * 3600
LONG_ROOT_WARNING = 30
"""Characters; above this, socket-binding tests (agentctl headless needs <= about 44) lose their margin."""
MOUNTINFO = Path("/proc/self/mountinfo")
_ROOT_NAME = re.compile(r"^(\d+)[a-z]?$")


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
def activate(*, keep_on_failure: bool = False) -> Iterator[Run]:
    """Point this process (and every child it starts) at a private root for the block.

    An inherited root is reused and left for its maker to remove; a root made here is removed
    when the block ends, unless the run failed and `keep_on_failure` is set (then it is kept and
    named). The previous values of the variables, and `tempfile`'s cached directory, are restored.
    """
    if not enabled():
        yield Run(root=None, owned=False)
        return
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
        if owned:
            if run.failed and keep_on_failure:
                print(f"private tmp: kept {root} after a failed run", file=sys.stderr)
            else:
                try:
                    remove_root(root)
                except (MountUnderRoot, OSError) as error:
                    print(f"private tmp: {error}", file=sys.stderr)
