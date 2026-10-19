"""Disk-image representation for wrkslots slots.

A slot normally stores its checkouts as ordinary directories on the host file
system. With the image representation, the whole slot directory is instead the
mount point of one sparse ext4 image file. The host sees a handful of files per
slot, whatever the agent writes inside it:

    <control>/slot-images/<slot-type>/<slot>/
        IMAGE.json   representation record (location, backend, ceiling)
        slot.img     sparse ext4 image mounted at the slot directory
        state.img    sparse ext4 image for the sandbox's per-slot private HOME directories
        state/       mount point of state.img

Why this helps: a slot's millions of small build files are metadata inside the
image rather than inside the host file system, a runaway writer fills its own
image instead of the host, and reclaim unmounts and deletes one file instead of
walking a tree.

The representation is a storage choice only. Lifecycle policy (ownership,
heartbeats, salvage, reclaim) is unchanged and lives in ``cli.py``.

Nothing here changes host configuration. Mounting uses either:

* ``kernel``: ``sudo -n mount -o loop`` (fast, needs passwordless sudo), or
* ``fuse``: ``fuse2fs`` as the invoking user (no privilege, slower metadata).

``auto`` picks ``kernel`` when ``sudo -n`` works and falls back to ``fuse``.
Images are sparse and are never pre-reserved: the ceiling is an upper bound on
the file's apparent size, not an allocation. Deleted blocks are returned to the
host (``discard`` for kernel mounts, ``fstrim`` on unmount for FUSE mounts).
"""

from __future__ import annotations

import dataclasses
import errno
import datetime as _dt
import hashlib
import json
import os
import select
import shutil
import stat as _stat
import struct
import subprocess
import sys
import time
import re
import uuid as _uuid
from collections.abc import Callable
from pathlib import Path
from typing import Mapping, Sequence

from wrkslots import sandbox

SCHEMA = "wrkslots-slot-image/v1"
IMAGES_DIRECTORY = "slot-images"
RECORD_NAME = "IMAGE.json"
SLOT_IMAGE_NAME = "slot.img"
STATE_IMAGE_NAME = "state.img"
STATE_MOUNT_NAME = "state"
BACKENDS = ("auto", "kernel", "fuse")
DEFAULT_CEILING_BYTES = 512 * 1024**3
DEFAULT_STATE_CEILING_BYTES = 128 * 1024**3
#: Entries mkfs creates that a fresh image may still contain. Anything else at
#: the image root is slot content and blocks destruction.
REPRESENTATION_RESIDUE = frozenset({"lost+found"})
#: The state image holds the sandbox's per-slot private $HOME directories
#: (home_private). /tmp is a fresh tmpfs per launch and is never stored.
STATE_SUBDIRECTORIES = ("home",)


class ImageError(RuntimeError):
    """A slot-image operation could not be completed safely."""


@dataclasses.dataclass(frozen=True)
class ImageSettings:
    """Configuration for newly created slot images."""

    ceiling_bytes: int = DEFAULT_CEILING_BYTES
    state_ceiling_bytes: int = DEFAULT_STATE_CEILING_BYTES
    backend: str = "auto"


#: The comment written above each ``image`` key in a literate configuration.
IMAGE_DOCS: dict[str, str] = {
    "ceiling_bytes": (
        f"Apparent size of each new slot image in bytes (default "
        f"{DEFAULT_CEILING_BYTES // 1024**3} GiB). Images are sparse: an upper bound, not a "
        "reservation. Raise one slot later with `wrkslots image grow`."
    ),
    "state_ceiling_bytes": (
        f"Apparent size of each slot's state image (its private $HOME layer) in bytes "
        f"(default {DEFAULT_STATE_CEILING_BYTES // 1024**3} GiB)."
    ),
    "backend": (
        "How slot images are mounted: kernel (sudo -n mount -o loop), fuse (fuse2fs, no "
        "privilege, slower for metadata-heavy work), or auto (default: kernel when sudo -n "
        "works, else fuse)."
    ),
}


@dataclasses.dataclass(frozen=True)
class SlotImage:
    """One slot's image directory and its durable representation record."""

    directory: Path
    slot: str
    slot_type: str
    location: Path
    ceiling_bytes: int
    state_ceiling_bytes: int
    backend: str
    created_at: str
    phase: str

    @property
    def slot_image(self) -> Path:
        """The sparse image mounted at the slot directory."""

        return self.directory / SLOT_IMAGE_NAME

    @property
    def state_image(self) -> Path:
        """The sparse image holding the sandbox's per-slot private HOME directories."""

        return self.directory / STATE_IMAGE_NAME

    @property
    def state_mount(self) -> Path:
        """Where the state image is mounted."""

        return self.directory / STATE_MOUNT_NAME

    @property
    def record_path(self) -> Path:
        """The durable representation record, IMAGE.json."""

        return self.directory / RECORD_NAME


@dataclasses.dataclass(frozen=True)
class MountEntry:
    """One line of /proc/self/mountinfo, reduced to what this module needs.

    ``device`` is the mounted file system's ``MAJOR:MINOR`` (field 3) and
    ``root`` the directory of that file system the mount shows (field 4): ``/``
    for a whole file system, a subdirectory for a bind mount of part of one.
    """

    mount_point: Path
    fstype: str
    source: str
    device: str = ""
    root: str = "/"


@dataclasses.dataclass(frozen=True)
class LoopEvidence:
    """What an unprivileged reader can learn about one bound loop device.

    ``backing`` is the backing file as sysfs renders it. That rendering is a
    path relative to the root of the mount namespace that attached the device,
    so once that namespace has exited it may not resolve at all, and it never
    identifies the file on its own. ``fsid`` is the file system id statfs
    reports for a mount of this device in this process's table; ext4 derives it
    from the superblock UUID, so it identifies the file system the device holds.
    ``udev_uuid`` is the file system UUID udev probed when the device was bound.
    """

    device: int
    name: str
    backing: str
    deleted: bool
    file_identity: tuple[int, int] | None
    fsid: int | None
    udev_uuid: str | None

    @property
    def label(self) -> str:
        """``/dev/loopN (MAJOR:MINOR)``, as messages name the device."""

        return f"{self.name} ({os.major(self.device)}:{os.minor(self.device)})"


@dataclasses.dataclass(frozen=True)
class ImageAttachments:
    """Which loop devices hold one image file, as far as an unprivileged reader can tell.

    ``devices`` carry this image's file system (proven by statfs, by a backing
    path that resolves to the image file, or by udev's UUID). ``host_device`` is
    the one mounted at the image's recorded mount point in this process's mount
    table, if it is one of them. More than one device means the image is
    attached twice: two independent file systems on one file. ``unidentified``
    lists bound loop devices whose image could not be determined at all.
    """

    image_file: Path
    mount_point: Path | None
    devices: tuple[LoopEvidence, ...]
    host_device: LoopEvidence | None
    unidentified: tuple[LoopEvidence, ...]

    @property
    def stale(self) -> tuple[LoopEvidence, ...]:
        """Every attachment but the host's own, when the image is attached twice."""

        if len(self.devices) < 2:
            return ()
        return tuple(item for item in self.devices if item != self.host_device)


# --------------------------------------------------------------------------- paths


def images_root(control: Path) -> Path:
    """Directory holding every slot's image directory."""

    return control / IMAGES_DIRECTORY


def image_directory(control: Path, slot_type: str, slot: str) -> Path:
    """Image directory for one slot."""

    return images_root(control) / slot_type / slot


def _now() -> str:
    return _dt.datetime.now(_dt.timezone.utc).replace(microsecond=0).isoformat()


# ------------------------------------------------------------------------ records


def _record_to_obj(image: SlotImage) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "slot": image.slot,
        "slot_type": image.slot_type,
        "location": str(image.location),
        "ceiling_bytes": image.ceiling_bytes,
        "state_ceiling_bytes": image.state_ceiling_bytes,
        "backend": image.backend,
        "created_at": image.created_at,
        "phase": image.phase,
    }


def _write_record(image: SlotImage) -> None:
    temporary = image.record_path.with_name(RECORD_NAME + ".tmp")
    data = json.dumps(_record_to_obj(image), indent=2, sort_keys=True) + "\n"
    with open(temporary, "w", encoding="utf-8") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, image.record_path)
    descriptor = os.open(image.directory, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _field(raw: Mapping[str, object], key: str, kind: type, path: Path) -> object:
    value = raw.get(key)
    if not isinstance(value, kind) or isinstance(value, bool):
        raise ImageError(f"slot image record {path} has an invalid {key!r}")
    return value


#: Per control directory, the function giving a slot's canonical directory
#: (``slot_type, slot -> path``). Registered by the configuration loader; an
#: image record's location is checked against it, never trusted as written.
_LAYOUTS: dict[Path, Callable[[str, str], Path]] = {}


def register_layout(control: Path, slot_directory: Callable[[str, str], Path]) -> None:
    """Tell image records where each slot of this control directory may live."""

    _LAYOUTS[Path(os.path.abspath(control))] = slot_directory


def _check_location(directory: Path, slot_type: str, slot: str, location: Path) -> None:
    """Refuse a recorded location wrkslots itself would not have chosen for this slot.

    Allowed: the slot's canonical directory; a sibling named ``.<slot>.<suffix>``
    (the path fences used by removal and recovery); and the conversion staging
    directory ``<images>/<type>/.<slot>.convert-<hex>``.
    """

    record = directory / RECORD_NAME
    if directory.name != slot or directory.parent.name != slot_type:
        raise ImageError(f"slot image record {record} names another slot ({slot_type}/{slot})")
    control = Path(os.path.abspath(directory.parent.parent.parent))
    layout = _LAYOUTS.get(control)
    if layout is None:
        raise ImageError(f"no slot layout is registered for {control}; load the configuration first")
    if not location.is_absolute() or os.path.normpath(location) != str(location):
        raise ImageError(f"slot image record {record} has a non-canonical location {location}")
    expected = Path(os.path.abspath(layout(slot_type, slot)))
    if location == expected:
        return
    sibling = re.fullmatch(rf"\.{re.escape(slot)}\.[A-Za-z0-9._-]+", location.name) is not None
    if sibling and location.parent == expected.parent:
        return
    staging = re.fullmatch(rf"\.{re.escape(slot)}\.convert-[0-9a-f]{{8}}", location.name) is not None
    if staging and location.parent == directory.parent:
        return
    raise ImageError(
        f"slot image record {record} places slot {slot_type}/{slot} at {location}, not at {expected}; "
        "refusing to mount or remove anything there"
    )


def read_record(directory: Path) -> SlotImage:
    """Read and validate one image directory's IMAGE.json (its location included)."""

    path = directory / RECORD_NAME
    try:
        raw_value: object = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ImageError(f"cannot read slot image record {path}: {exc}") from exc
    if not isinstance(raw_value, dict):
        raise ImageError(f"slot image record {path} is not an object")
    raw: dict[str, object] = {str(key): value for key, value in raw_value.items()}
    if raw.get("schema") != SCHEMA:
        raise ImageError(f"slot image record {path} has unsupported schema {raw.get('schema')!r}")
    location = Path(str(_field(raw, "location", str, path)))
    if not location.is_absolute():
        raise ImageError(f"slot image record {path} has a relative location")
    _check_location(directory, str(_field(raw, "slot_type", str, path)), str(_field(raw, "slot", str, path)), location)
    return SlotImage(
        directory=directory,
        slot=str(_field(raw, "slot", str, path)),
        slot_type=str(_field(raw, "slot_type", str, path)),
        location=location,
        ceiling_bytes=int(str(_field(raw, "ceiling_bytes", int, path))),
        state_ceiling_bytes=int(str(_field(raw, "state_ceiling_bytes", int, path))),
        backend=str(_field(raw, "backend", str, path)),
        created_at=str(_field(raw, "created_at", str, path)),
        phase=str(_field(raw, "phase", str, path)),
    )


def load(control: Path, slot_type: str, slot: str) -> SlotImage | None:
    """Return the slot's image record, or None when the slot is not image-backed."""

    directory = image_directory(control, slot_type, slot)
    if not (directory / RECORD_NAME).exists():
        return None
    return read_record(directory)


def all_images(control: Path) -> list[SlotImage]:
    """Every image record under the project's control directory."""

    root = images_root(control)
    found: list[SlotImage] = []
    if not root.is_dir():
        return found
    for type_directory in sorted(root.iterdir()):
        if not type_directory.is_dir() or type_directory.is_symlink():
            continue
        for directory in sorted(type_directory.iterdir()):
            if (directory / RECORD_NAME).exists():
                found.append(read_record(directory))
    return found


def image_for_location(control: Path, location: Path) -> SlotImage | None:
    """Return the image whose recorded location is exactly ``location``."""

    target = Path(os.path.abspath(location))
    for image in all_images(control):
        if Path(os.path.abspath(image.location)) == target:
            return image
    return None


# ------------------------------------------------------------------ subprocesses


def _run(argv: Sequence[str], *, cwd: Path | None = None, timeout: float = 120.0) -> str:
    try:
        result = subprocess.run(
            list(argv),
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ImageError(f"{argv[0]} failed to run: {exc}") from exc
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip().splitlines()
        raise ImageError(
            f"{' '.join(argv[:3])} exited {result.returncode}: "
            f"{detail[-1] if detail else 'no output'}"
        )
    return result.stdout


_IN_BOX_NAMESPACE: bool | None = None


def _spec_directory_is_mounted() -> bool:
    """Whether the root launcher's spec directory is a mount point here (true only in a box)."""

    global _IN_BOX_NAMESPACE
    if _IN_BOX_NAMESPACE is None:
        directory = sandbox.spec_directory()
        found = False
        if directory is not None:
            try:
                found = any(entry.mount_point == directory for entry in mount_table())
            except ImageError:
                found = False
        _IN_BOX_NAMESPACE = found
    return _IN_BOX_NAMESPACE


def box_kind() -> str | None:
    """None outside any wrkslots box; ``coordinator`` or ``slot`` inside one.

    A box binds the root launcher's spec directory onto itself, so that path is
    a mount point only inside a box, and a box sets WRKSLOTS_SANDBOX; a
    coordinator box (`wrkslots box`) also sets WRKSLOTS_BOX. Limits-only
    (cgroup) runs stay in the host namespace and count as no box.
    """

    sandboxed = os.environ.get("WRKSLOTS_SANDBOX")
    if sandboxed == "cgroup":
        return None
    if sandboxed not in ("userns", "root") and not _spec_directory_is_mounted():
        return None
    return "coordinator" if os.environ.get("WRKSLOTS_BOX") else "slot"


def in_box() -> bool:
    """Whether this process runs in a wrkslots box's own mount namespace."""

    return box_kind() is not None


_BOX_MOUNT_HINT = (
    "image mounts requested from inside a coordinator box run in the host's mount namespace "
    "through the user's systemd manager (systemd-run --user); if that is unavailable, create "
    "and remove image slots outside the box"
)
_SLOT_BOX_REFUSAL = (
    "slot images cannot be mounted, unmounted, converted, or removed from inside a slot's box "
    "(its mounts are private to it); run this outside the box, or from a coordinator box "
    "(`wrkslots box`)"
)


def _via_user_manager(anchor: Path | None = None) -> bool:
    """Whether a mount operation must run in the host namespace through the user manager.

    False outside any box (the operation runs here). True in a coordinator box,
    whose view receives the host's mounts under the worktrees directory. A
    slot's box keeps its mounts private, so a mount made on the host would be
    invisible to it and one made in it invisible to everyone else: refused.
    ``anchor`` (an image directory) must also be writable here, which only a
    coordinator box allows.
    """

    kind = box_kind()
    if kind is None:
        return False
    if kind == "coordinator" and (anchor is None or os.access(anchor, os.W_OK)):
        return True
    raise ImageError(_SLOT_BOX_REFUSAL)


def _user_manager_argv(argv: Sequence[str]) -> list[str]:
    systemd_run = shutil.which("systemd-run")
    if systemd_run is None:
        raise ImageError(f"systemd-run is not installed; {_BOX_MOUNT_HINT}")
    return [
        systemd_run,
        "--user",
        "--quiet",
        "--collect",
        "--wait",
        "--pipe",
        "--service-type=exec",
        "--",
        *argv,
    ]


def _host_argv(argv: Sequence[str], anchor: Path | None = None) -> list[str]:
    """``argv``, run in the host's mount namespace.

    Outside a box that is simply here. In a coordinator box, a mount made here
    would land in the box's own namespace, invisible to the host and to every
    worker launched into the slot, so the command runs as a transient service of
    the user's systemd manager, which lives in the host namespace; the box's
    slave propagation then shows the result inside the box too.
    """

    return _user_manager_argv(argv) if _via_user_manager(anchor) else list(argv)


#: The host mount table as read through the user manager, while nothing has
#: changed it since (every host-side operation forgets it).
_HOST_TABLE: list[MountEntry] | None = None


def _forget_host_table() -> None:
    global _HOST_TABLE
    _HOST_TABLE = None


def verification_table() -> list[MountEntry]:
    """The mount table that decides whether an image is mounted: the host's.

    Outside a box that is this process's own table. In a coordinator box it is
    read in the host namespace through the user manager, because the box's own
    table can keep copies of mounts that are gone on the host (a copy under a
    mount whose propagation the box cut off) and so cannot verify a host-side
    operation.
    """

    global _HOST_TABLE
    if box_kind() != "coordinator":
        return mount_table()
    if _HOST_TABLE is None:
        cat = shutil.which("cat") or "/bin/cat"
        text = _run(_user_manager_argv([cat, "/proc/self/mountinfo"]))
        _HOST_TABLE = _parse_mount_table(text)
    return _HOST_TABLE


def _run_on_host(argv: Sequence[str], *, anchor: Path | None = None, timeout: float = 120.0) -> str:
    routed = _via_user_manager(anchor)
    try:
        return _run(_user_manager_argv(argv) if routed else list(argv), timeout=timeout)
    except ImageError as exc:
        if routed:
            raise ImageError(f"{exc} ({_BOX_MOUNT_HINT})") from exc
        raise
    finally:
        _forget_host_table()


_PATH_OPERATION = """\
import os, stat, sys
operation, *paths = sys.argv[1:]
if operation == "rmdir":
    os.rmdir(paths[0])
elif operation == "rename":
    os.rename(paths[0], paths[1])
else:  # replace: a fresh empty directory with the same mode takes the old one's place
    target = paths[0]
    info = os.lstat(target)
    if not stat.S_ISDIR(info.st_mode):
        raise SystemExit(f"{target} is not a directory")
    parent, name = os.path.split(target)
    fresh = os.path.join(parent, f".{name}.replace-{os.getpid()}")
    os.mkdir(fresh, stat.S_IMODE(info.st_mode))
    try:
        os.chmod(fresh, stat.S_IMODE(info.st_mode))
        os.rename(fresh, target)
    except BaseException:
        os.rmdir(fresh)
        raise
"""


def _host_path_operation(operation: str, *paths: Path, anchor: Path | None) -> None:
    """rmdir or rename a slot mount point, in the host namespace when in a coordinator box.

    A box can keep copies of a slot's mount that the host has already removed
    (see :func:`verification_table`); the kernel refuses to remove or rename a
    directory that is a mount point in the caller's own namespace, but detaches
    such copies when the host removes it. Outside a box this is a plain call.
    """

    if not _via_user_manager(anchor):
        if operation == "rmdir":
            os.rmdir(paths[0])
        elif operation == "rename":
            os.rename(paths[0], paths[1])
        else:
            _run([os.path.realpath(sys.executable), "-I", "-c", _PATH_OPERATION, operation, *map(str, paths)])
        return
    try:
        _run(_user_manager_argv([os.path.realpath(sys.executable), "-I", "-c", _PATH_OPERATION, operation, *map(str, paths)]))
    except ImageError as exc:
        raise OSError(errno.EBUSY, f"{operation} {' '.join(map(str, paths))} on the host failed: {exc}") from exc
    finally:
        _forget_host_table()


def remove_mount_point(path: Path, anchor: Path | None = None) -> None:
    """Remove an empty slot mount-point directory (on the host from a coordinator box)."""

    _host_path_operation("rmdir", path, anchor=anchor)


def release_mount_point_copies(mount_point: Path, anchor: Path | None = None) -> None:
    """Detach every copy of an unmounted slot mount that another namespace still holds.

    The kernel does not propagate an unmount into every mount namespace: a box
    keeps copies under parents it made private, and so can unrelated namespaces
    on the host. Such a copy keeps a FUSE server alive and a loop device bound.
    Replacing the (empty, now unmounted) mount-point directory with a fresh one
    removes its old directory entry, and the kernel detaches every mount on a
    removed entry, in every namespace. Runs on the host from a coordinator box.
    """

    try:
        _host_path_operation("replace", mount_point, anchor=anchor)
    except (OSError, ImageError) as exc:
        # Best effort: the caller's own checks decide what failed. Say so, so a
        # copy left behind (a bound loop device, a lingering fuse2fs) has a trace.
        print(f"wrkslots: note: could not release stale copies of {mount_point}: {exc}", file=sys.stderr)


def rename_mount_point(source: Path, destination: Path, anchor: Path | None = None) -> None:
    """Rename an unmounted slot mount-point directory (on the host from a coordinator box)."""

    _host_path_operation("rename", source, destination, anchor=anchor)


def _privileged_argv() -> list[str]:
    helper = Path(__file__).resolve().with_name("imagehelper.py")
    sudo = shutil.which("sudo") or "sudo"
    return [sudo, "-n", "--", os.path.realpath(sys.executable), "-I", str(helper)]


def _privileged(operation: str, *arguments: str, anchor: Path | None = None, timeout: float = 120.0) -> str:
    """Run one kernel-backend operation through the checking helper as root, in the host namespace."""

    return _run_on_host([*_privileged_argv(), operation, *arguments], anchor=anchor, timeout=timeout)


def sudo_available() -> bool:
    """Whether `sudo -n` works without a password prompt (where image mounts run)."""

    try:
        argv = _host_argv([shutil.which("sudo") or "sudo", "-n", "true"])
        return subprocess.run(argv, capture_output=True, timeout=30, check=False).returncode == 0
    except (OSError, subprocess.TimeoutExpired, ImageError):
        return False


def _wait_until(condition: Callable[[], bool], timeout: float) -> bool:
    """Poll ``condition`` (host-side in a coordinator box) until it holds or ``timeout`` passes."""

    interval = 0.25 if box_kind() == "coordinator" else 0.05
    deadline = time.monotonic() + timeout
    while True:
        _forget_host_table()
        if condition():
            return True
        if time.monotonic() > deadline:
            return False
        time.sleep(interval)


def fuse_available() -> bool:
    """Whether fuse2fs and a usable /dev/fuse are present."""

    return shutil.which("fuse2fs") is not None and os.access("/dev/fuse", os.R_OK | os.W_OK)


def resolve_backend(preference: str) -> str:
    """Pick the concrete backend for ``preference`` on this host, or refuse."""

    override = os.environ.get("WRKSLOTS_IMAGE_BACKEND")
    if override:
        preference = override
    if preference not in BACKENDS:
        raise ImageError(f"image backend must be one of {', '.join(BACKENDS)}, not {preference!r}")
    _via_user_manager()  # a slot's box refuses before any backend is probed
    if preference in ("auto", "kernel") and sudo_available():
        return "kernel"
    if preference == "kernel":
        raise ImageError("kernel image backend needs passwordless sudo (sudo -n true failed)")
    if fuse_available():
        return "fuse"
    raise ImageError(
        "no image backend is usable: sudo -n is unavailable and fuse2fs or /dev/fuse is missing"
    )


# ---------------------------------------------------------------------- mountinfo


def _unescape(value: str) -> str:
    for escaped, character in (("\\040", " "), ("\\011", "\t"), ("\\012", "\n"), ("\\134", "\\")):
        value = value.replace(escaped, character)
    return value


def mount_table(mountinfo: Path = Path("/proc/self/mountinfo")) -> list[MountEntry]:
    """Parse the calling process's mount table."""

    try:
        text = mountinfo.read_text(encoding="utf-8", errors="surrogateescape")
    except OSError as exc:
        raise ImageError(f"cannot read {mountinfo}: {exc}") from exc
    return _parse_mount_table(text)


def _parse_mount_table(text: str) -> list[MountEntry]:
    entries: list[MountEntry] = []
    for line in text.splitlines():
        left, separator, right = line.partition(" - ")
        if not separator:
            continue
        left_fields = left.split()
        right_fields = right.split()
        if len(left_fields) < 5 or len(right_fields) < 2:
            continue
        entries.append(
            MountEntry(
                mount_point=Path(_unescape(left_fields[4])),
                fstype=right_fields[0],
                source=_unescape(right_fields[1]),
                device=left_fields[2],
                root=_unescape(left_fields[3]),
            )
        )
    return entries


# ------------------------------------------------------------- loop-device identity
#
# ⚠️ A LOOP DEVICE'S BACKING FILE IS IDENTIFIED BY CONTENT, NEVER BY ITS RENDERED
# PATH. sysfs renders the backing file relative to the root of the mount
# namespace that attached the device. When that namespace has exited (a
# per-session namespace, a sandbox), the rendering is relative to a detached
# mount's root and does not resolve anywhere. Comparing those paths made a
# slot's own mount look foreign: the process-use census reported it as use,
# unmount returned as if nothing were mounted, and every command warned that
# the mount point held "a different file system" (#350 wrkslots-foreign-mount-ns).
#
# What identifies the file system a device holds, without privilege: statfs of
# a mount of it (ext4 derives f_fsid from the superblock UUID, which mkfs makes
# for every image and which can be read from the user-owned image file), a
# backing rendering that does resolve to the image file's own device and inode,
# and udev's probed UUID. A backing file shown as deleted is never the image
# file that exists at that path now.

_LOOP_MAJOR = 7
_EXT4_ROOT_INODE = 2
_SUPERBLOCK_OFFSET = 1024
_SUPERBLOCK_PREFIX = 0x78
_EXT4_MAGIC = 0xEF53
#: Where udev keeps what it probed for each block device (world-readable).
UDEV_DATA = Path("/run/udev/data")
#: Where the kernel lists block devices (a loop device is bound while it has loop/backing_file).
SYS_BLOCK = Path("/sys/block")


def image_uuid(image_file: Path) -> str | None:
    """The ext4 UUID in an image file's superblock, read as the invoking user, or None."""

    try:
        descriptor = os.open(image_file, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        return None
    try:
        block = os.pread(descriptor, _SUPERBLOCK_PREFIX, _SUPERBLOCK_OFFSET)
    except OSError:
        return None
    finally:
        os.close(descriptor)
    if len(block) < _SUPERBLOCK_PREFIX or struct.unpack_from("<H", block, 0x38)[0] != _EXT4_MAGIC:
        return None
    return str(_uuid.UUID(bytes=bytes(block[0x68:0x78])))


def uuid_fsid(uuid_text: str) -> int:
    """The ``statvfs().f_fsid`` ext4 reports for a file system with this UUID."""

    low, high = struct.unpack("<QQ", _uuid.UUID(uuid_text).bytes)
    return int(low ^ high)


def parse_device(text: str) -> int | None:
    """``MAJOR:MINOR`` as a device number, or None when it is not one."""

    major, separator, minor = text.partition(":")
    if not separator or not major.isdigit() or not minor.isdigit():
        return None
    return os.makedev(int(major), int(minor))


def device_text(device: int) -> str:
    """A device number as ``MAJOR:MINOR``, the form mountinfo uses."""

    return f"{os.major(device)}:{os.minor(device)}"


def _file_identity(path: Path) -> tuple[int, int] | None:
    try:
        info = os.stat(path)
    except OSError:
        return None
    return info.st_dev, info.st_ino


def _same_file(left: Path, right: Path) -> bool:
    identity = _file_identity(left)
    return identity is not None and identity == _file_identity(right)


def _mounted_fsid(path: Path, device: int) -> int | None:
    """statfs of the file system rooted at ``path``, when that root is ``device``'s."""

    try:
        descriptor = os.open(path, os.O_PATH | os.O_NOFOLLOW | os.O_DIRECTORY | os.O_CLOEXEC)
    except OSError:
        return None
    try:
        info = os.fstat(descriptor)
        if info.st_dev != device or info.st_ino != _EXT4_ROOT_INODE:
            return None
        return int(os.fstatvfs(descriptor).f_fsid)
    except OSError:
        return None
    finally:
        os.close(descriptor)


def _udev_uuid(device: int) -> str | None:
    try:
        text = (UDEV_DATA / f"b{device_text(device)}").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    for line in text.splitlines():
        if line.startswith("E:ID_FS_UUID="):
            value = line.split("=", 1)[1].strip().lower()
            try:
                return str(_uuid.UUID(value))
            except ValueError:
                return None
    return None


def loop_evidence(table: Sequence[MountEntry] | None = None) -> dict[int, LoopEvidence]:
    """Every bound loop device and what identifies the file system it holds."""

    try:
        names = sorted(os.listdir(SYS_BLOCK))
    except OSError:
        return {}
    entries = mount_table() if table is None else table
    places: dict[int, list[Path]] = {}
    for entry in entries:
        number = parse_device(entry.device)
        if number is not None and os.major(number) == _LOOP_MAJOR and entry.root == "/":
            places.setdefault(number, []).append(entry.mount_point)
    found: dict[int, LoopEvidence] = {}
    for name in names:
        if not name.startswith("loop") or not name[4:].isdigit():
            continue
        try:
            backing = (SYS_BLOCK / name / "loop" / "backing_file").read_text(
                encoding="utf-8", errors="surrogateescape"
            ).removesuffix("\n")
            number = parse_device(
                (SYS_BLOCK / name / "dev").read_text(encoding="ascii").strip()
            )
        except OSError:
            continue  # not bound (no backing file) or gone meanwhile
        if number is None:
            continue
        deleted = backing.endswith(" (deleted)")
        fsid: int | None = None
        for place in places.get(number, ()):
            fsid = _mounted_fsid(place, number)
            if fsid is not None:
                break
        found[number] = LoopEvidence(
            device=number,
            name=f"/dev/{name}",
            backing=backing,
            deleted=deleted,
            file_identity=None if deleted else _file_identity(Path(backing)),
            fsid=fsid,
            udev_uuid=_udev_uuid(number),
        )
    return found


def _carries(evidence: LoopEvidence, identity: tuple[int, int] | None, uuid_text: str | None) -> bool | None:
    """True when the device provably holds this image, False when provably not, else None."""

    if evidence.fsid is not None and uuid_text is not None:
        return evidence.fsid == uuid_fsid(uuid_text)
    if evidence.deleted:
        return False
    if identity is not None and evidence.file_identity == identity:
        return True
    if evidence.udev_uuid is not None and uuid_text is not None:
        return evidence.udev_uuid == uuid_text
    return None


def _top_entry(mount_point: Path, entries: Sequence[MountEntry]) -> MountEntry | None:
    target = Path(os.path.abspath(mount_point))
    found: MountEntry | None = None
    for entry in entries:
        if entry.mount_point == target:
            found = entry
    return found


def image_attachments(
    image_file: Path,
    mount_point: Path | None = None,
    *,
    table: Sequence[MountEntry] | None = None,
    evidence: Mapping[int, LoopEvidence] | None = None,
) -> ImageAttachments:
    """Which bound loop devices hold ``image_file``; see :class:`ImageAttachments`."""

    entries = mount_table() if table is None else table
    loops = loop_evidence(entries) if evidence is None else evidence
    identity = _file_identity(image_file)
    uuid_text = image_uuid(image_file)
    devices: list[LoopEvidence] = []
    unidentified: list[LoopEvidence] = []
    for item in loops.values():
        verdict = _carries(item, identity, uuid_text)
        if verdict is True:
            devices.append(item)
        elif verdict is None:
            unidentified.append(item)
    host: LoopEvidence | None = None
    top = None if mount_point is None else _top_entry(mount_point, entries)
    if top is not None and top.fstype == "ext4" and top.root == "/":
        number = parse_device(top.device)
        host = next((item for item in devices if item.device == number), None)
    return ImageAttachments(
        image_file=image_file,
        mount_point=None if mount_point is None else Path(os.path.abspath(mount_point)),
        devices=tuple(devices),
        host_device=host,
        unidentified=tuple(unidentified),
    )


def mount_state(image_file: Path, mount_point: Path, table: Sequence[MountEntry] | None = None) -> tuple[str, str]:
    """What is mounted at ``mount_point``, judged against ``image_file``.

    Returns ``(state, detail)``. ``state`` is ``absent`` (nothing is mounted
    there), ``kernel`` or ``fuse`` (this image, by that backend), ``other`` (a
    file system that is provably not this image's own mount, including a bind
    of a subdirectory) or ``unverifiable`` (a loop-device ext4 mount whose image
    cannot be determined). Every check reads the host's mount table
    (:func:`verification_table`) unless ``table`` is given.
    """

    entries = table if table is not None else verification_table()
    top = _top_entry(mount_point, entries)
    if top is None:
        return "absent", "nothing is mounted there"
    what = f"{top.fstype} {top.source} ({top.device}, root {top.root})"
    if top.fstype.startswith("fuse"):
        if _same_file(Path(top.source), image_file):
            return "fuse", what
        return "other", what
    number = parse_device(top.device)
    if top.fstype != "ext4" or number is None or os.major(number) != _LOOP_MAJOR:
        return "other", what
    if top.root != "/":
        return "other", f"{what}: a bind of part of a file system, not an image's own mount"
    uuid_text = image_uuid(image_file)
    if uuid_text is not None:
        fsid = _mounted_fsid(Path(os.path.abspath(mount_point)), number)
        if fsid is not None:
            return ("kernel", what) if fsid == uuid_fsid(uuid_text) else ("other", what)
    item = loop_evidence(entries).get(number)
    if item is None:
        return "unverifiable", f"{what}: the loop device is not bound"
    verdict = _carries(item, _file_identity(image_file), uuid_text)
    if verdict is True:
        return "kernel", what
    if verdict is False:
        return "other", what
    return (
        "unverifiable",
        f"{what}: cannot tell which image {item.name} holds (its backing file renders as "
        f"{item.backing!r}, which does not resolve here, and udev has no file system UUID for it)",
    )


def mounted_backend(image_file: Path, mount_point: Path, table: Sequence[MountEntry] | None = None) -> str | None:
    """``kernel`` or ``fuse`` when exactly ``image_file`` is mounted at ``mount_point``, else None."""

    state, _detail = mount_state(image_file, mount_point, table)
    return state if state in ("kernel", "fuse") else None


def is_mounted(image_file: Path, mount_point: Path, table: Sequence[MountEntry] | None = None) -> bool:
    """Whether exactly ``image_file`` is mounted at ``mount_point``."""

    return mounted_backend(image_file, mount_point, table) is not None


def project_attachments(control: Path) -> list[ImageAttachments]:
    """Attachments of every slot and state image of the project (one loop scan)."""

    try:
        images = all_images(control)
        table = mount_table()
    except ImageError:
        return []
    loops = loop_evidence(table)
    found: list[ImageAttachments] = []
    for image in images:
        for image_file, mount_point in (
            (image.slot_image, image.location),
            (image.state_image, image.state_mount),
        ):
            found.append(image_attachments(image_file, mount_point, table=table, evidence=loops))
    return found


def owned_mounts(control: Path) -> tuple[frozenset[tuple[Path, str]], frozenset[tuple[Path, str, str]]]:
    """This project's own image mounts, as the process-use census must see them.

    Returns ``(owned, stale)``. ``owned`` holds ``(mount point, MAJOR:MINOR)``
    for each device that is a slot's own storage at its recorded location: the
    one device holding the image, or, when the image is attached twice, the one
    this process's table mounts there. A copy of such a mount in another
    namespace (same device, mount root ``/``, same path) is the same file system
    and is not use. ``stale`` holds ``(mount point, MAJOR:MINOR, /dev/loopN)``
    for every other attachment of a doubly attached image: a second, independent
    file system on the same file, which salvage cannot see and which the census
    therefore keeps reporting. For an image in :data:`DISCARD_PHASE`, only the
    host's own mount is storage: every other device that holds it may hold the
    stale file system, even when it is the only device left.
    """

    owned: set[tuple[Path, str]] = set()
    stale: set[tuple[Path, str, str]] = set()
    try:
        images = all_images(control)
        table = mount_table()
    except ImageError:
        return frozenset(), frozenset()
    loops = loop_evidence(table)
    for image in images:
        for image_file, mount_point in (
            (image.slot_image, image.location),
            (image.state_image, image.state_mount),
        ):
            target = Path(os.path.abspath(mount_point))
            found = image_attachments(image_file, target, table=table, evidence=loops)
            if image.phase == DISCARD_PHASE:
                if found.host_device is not None:
                    owned.add((target, device_text(found.host_device.device)))
                for item in found.devices:
                    if item != found.host_device:
                        stale.add((target, device_text(item.device), item.name))
            else:
                if len(found.devices) == 1:
                    owned.add((target, device_text(found.devices[0].device)))
                elif found.host_device is not None:
                    owned.add((target, device_text(found.host_device.device)))
                for item in found.stale:
                    stale.add((target, device_text(item.device), item.name))
            for entry in table:
                if (
                    entry.mount_point == target
                    and entry.fstype.startswith("fuse")
                    and _same_file(Path(entry.source), image_file)
                ):
                    owned.add((target, entry.device))
    return frozenset(owned), frozenset(stale)


# ---------------------------------------------------------------- image lifecycle


def make_image_file(path: Path, size_bytes: int) -> None:
    """Create a sparse, no-copy-on-write ext4 image owned by the invoking user."""

    if path.exists() or path.is_symlink():
        raise ImageError(f"image file already exists: {path}")
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(descriptor)
    # No-copy-on-write keeps a btrfs host from fragmenting the image into one
    # extent per guest write. It must be set while the file is empty; hosts that
    # are not btrfs reject it harmlessly.
    subprocess.run(["chattr", "+C", str(path)], capture_output=True, check=False)
    os.truncate(path, size_bytes)
    _run(
        [
            "mkfs.ext4",
            "-q",
            "-F",
            "-m",
            "0",
            "-J",
            "size=64",
            "-E",
            f"root_owner={os.getuid()}:{os.getgid()},nodiscard,lazy_itable_init=1",
            str(path),
        ],
        timeout=300,
    )


def _fuse_mount(image_file: Path, mount_point: Path) -> None:
    routed = _via_user_manager(image_file.parent)
    argv = ["fuse2fs", str(image_file), str(mount_point)]
    # Run the FUSE server in its own transient user service so it outlives the
    # command (or agent sandbox) that mounted it. A user service also runs in
    # the host's mount namespace, which a coordinator box needs.
    if shutil.which("systemd-run") is not None:
        digest = hashlib.sha256(os.path.abspath(image_file).encode()).hexdigest()[:12]
        unit = f"wrkslots-fuse-{digest}"
        # Nested in the caller's own enclosing slice, like every slot slice:
        # wrkslots never moves work out of a slice site policy placed it in.
        images_slice = sandbox.root_slice().removesuffix(".slice") + "-images.slice"
        probe = subprocess.run(
            [
                "systemd-run",
                "--user",
                "--quiet",
                "--collect",
                f"--unit={unit}-{os.getpid()}-{int(time.time())}",
                f"--slice={images_slice}",
                f"--working-directory={image_file.parent}",
                "--",
                "fuse2fs",
                "-f",
                str(image_file),
                str(mount_point),
            ],
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
        _forget_host_table()
        if probe.returncode == 0:
            if _wait_until(lambda: is_mounted(image_file, mount_point), 30):
                return
            raise ImageError(f"fuse2fs did not mount {image_file} at {mount_point} within 30s")
    if routed:
        # A daemonizing fuse2fs run here would mount into the box's own namespace.
        raise ImageError(f"cannot start fuse2fs as a user service for {image_file}; {_BOX_MOUNT_HINT}")
    _run(argv, cwd=image_file.parent)
    if not is_mounted(image_file, mount_point):
        raise ImageError(f"fuse2fs exited without mounting {image_file} at {mount_point}")


def _undo_mount(image_file: Path, mount_point: Path) -> str:
    """Best-effort removal of a mount that failed its check; returns what happened."""

    try:
        unmount(image_file, mount_point)
    except (ImageError, OSError) as exc:
        return f"; undoing it failed too: {exc}"
    return "; it was unmounted again"


#: The device the last kernel mount reused instead of attaching a new one, by
#: image file. :func:`ensure_mounted` reports it; nothing else depends on it.
_REUSED_DEVICES: dict[Path, str] = {}


def mount(image_file: Path, mount_point: Path, backend_preference: str, *, fresh: bool = False) -> str:
    """Mount ``image_file`` at ``mount_point``; return the backend used.

    Every check reads the host's mount table (:func:`verification_table`). A
    mount that was made but fails its check is undone before the refusal.

    The kernel backend never attaches an image to a second loop device: when a
    device already holds it (a copy of an earlier mount in another mount
    namespace keeps it bound), the root helper mounts that same device, so
    there is still one file system; when two devices hold it, it refuses.
    With ``fresh`` the helper refuses whenever any device already holds the
    image, judged by the kernel's own record of each device's backing file:
    an image whose stale copy is being discarded is mounted only from the
    image file, never through a device that copy may still hold.
    """

    _REUSED_DEVICES.pop(image_file, None)
    state, detail = mount_state(image_file, mount_point)
    if state in ("kernel", "fuse"):
        return state
    if state != "absent" or os.path.ismount(mount_point):
        raise ImageError(f"{mount_point} already has a different file system mounted on it: {detail}")
    if mount_point.is_symlink() or not mount_point.is_dir():
        raise ImageError(f"image mount point is not a real directory: {mount_point}")
    content = sorted(entry.name for entry in mount_point.iterdir())
    if content:
        raise ImageError(
            f"image mount point is not empty: {mount_point} holds {', '.join(content[:5])}"
        )
    backend = resolve_backend(backend_preference)
    if _fuse_servers(image_file):
        raise ImageError(f"a fuse2fs process still serves {image_file}; refusing a second mount")
    try:
        if backend == "kernel":
            output = _privileged(
                "mount",
                str(image_file),
                str(mount_point),
                *(("--fresh",) if fresh else ()),
                anchor=image_file.parent,
            )
            for line in output.splitlines():
                if line.startswith("reused "):
                    _REUSED_DEVICES[image_file] = line.removeprefix("reused ").strip()
        else:
            _fuse_mount(image_file, mount_point)
    except ImageError as exc:
        _forget_host_table()
        if is_mounted(image_file, mount_point):
            raise ImageError(f"{exc}{_undo_mount(image_file, mount_point)}") from exc
        raise
    _forget_host_table()
    if not is_mounted(image_file, mount_point):
        state, detail = mount_state(image_file, mount_point)
        undone = _undo_mount(image_file, mount_point) if state != "absent" else ""
        raise ImageError(
            f"mount reported success but {image_file} is not mounted at {mount_point} "
            f"({detail}){undone}"
        )
    return backend


def _fuse_servers(image_file: Path) -> list[int]:
    """PIDs of fuse2fs processes serving ``image_file`` (one scan of /proc; slow on busy hosts)."""

    target = os.path.abspath(image_file)
    found: list[int] = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/cmdline", "rb") as handle:
                argv = handle.read().split(b"\0")
        except OSError:
            continue
        if not argv or not os.path.basename(argv[0]).startswith(b"fuse2fs"):
            continue
        try:
            cwd = os.readlink(f"/proc/{entry}/cwd")
        except OSError:
            cwd = ""
        for raw in argv[1:]:
            text = raw.decode(errors="surrogateescape")
            if text and os.path.abspath(os.path.join(cwd, text)) == target:
                found.append(int(entry))
                break
    return found


def _wait_for_exit(pids: Sequence[int], timeout: float) -> bool:
    """Wait until every process in ``pids`` has exited; False on timeout.

    Each process is pinned by a pidfd where the kernel offers one, so a reused
    PID is never mistaken for the old process; otherwise it is probed with
    signal 0.
    """

    handles: list[tuple[int, int | None]] = []
    for pid in pids:
        try:
            handles.append((pid, os.pidfd_open(pid)))
        except ProcessLookupError:
            continue
        except (AttributeError, OSError):
            handles.append((pid, None))
    deadline = time.monotonic() + timeout
    try:
        while handles:
            remaining: list[tuple[int, int | None]] = []
            for pid, descriptor in handles:
                if descriptor is not None:
                    alive = not select.select([descriptor], [], [], 0)[0]
                else:
                    try:
                        os.kill(pid, 0)
                        alive = True
                    except ProcessLookupError:
                        alive = False
                    except PermissionError:
                        alive = True
                if alive:
                    remaining.append((pid, descriptor))
                elif descriptor is not None:
                    os.close(descriptor)
            handles = remaining
            if not handles:
                return True
            if time.monotonic() > deadline:
                return False
            time.sleep(0.05)
        return True
    finally:
        for _pid, descriptor in handles:
            if descriptor is not None:
                os.close(descriptor)


#: Image files whose second, stale loop attachment the current command may
#: discard: only a single-slot remove given --discard-stale-image-copy, after
#: salvage read the host's own file system, sets this (see cli).
DISCARD_STALE_COPIES: set[Path] = set()

#: The phase of an image whose stale copy is being discarded. Its record enters
#: it, durably, before the host's own mount is unmounted (by :func:`relocate`
#: or :func:`destroy`), and keeps it while the image may be unmounted with a
#: loop device that a stale copy holds. Mounting such an image would reuse that
#: device, and so put the stale file system where the slot's content was, so
#: nothing mounts it automatically (:func:`ensure_mounted`), and the census
#: counts every device holding it but the host's own mount as stale
#: (:func:`owned_mounts`). It leaves the phase only through a mount made from
#: the image file itself: :func:`relocate` once no device holds it, or
#: ``image mount --coordinator-authorized``.
DISCARD_PHASE = "discarding-stale-copy"


def discard_hold_message(image: SlotImage, held: Sequence[str]) -> str:
    """Why an image in :data:`DISCARD_PHASE` is not mounted, naming the devices ``held`` by it."""

    return (
        f"slot image {image.slot_image} was unmounted while a stale copy of it was being "
        "discarded"
        + (f", and {', '.join(held)} still holds it" if held else "")
        + "; it is not mounted again: a mount would reuse a loop device that may hold the "
        "stale file system, and a stale copy released after the host's own file system may "
        "have written its old metadata into the image. remedy: stop the processes whose mount "
        "table holds that device; once no loop device holds the image, check the image file "
        "(e2fsck -fn) and run 'wrkslots image mount --coordinator-authorized', which mounts it "
        "from the file only"
    )


def still_attached(image_file: Path) -> tuple[LoopEvidence, ...]:
    """Loop devices that provably still hold ``image_file`` (best effort, unprivileged)."""

    return image_attachments(image_file).devices


def unmount(image_file: Path, mount_point: Path) -> list[str]:
    """Unmount ``image_file`` from ``mount_point`` if it is mounted there.

    Idempotent: an image that is not mounted there is left alone, so a
    sequence interrupted halfway (state image unmounted, slot image not) is
    finished by running it again. The result is verified in the host's mount
    table. Anything else at the mount point refuses: a different file system,
    or a loop mount whose image cannot be identified, is never taken for
    "not mounted" (the caller would then delete or move an image that is still
    in use).

    A copy of the mount in another mount namespace keeps the loop device bound
    after the host unmount. It is released by replacing the empty mount-point
    directory (:func:`release_mount_point_copies`). Returns the devices that
    provably still hold the image afterwards (a copy that is busy keeps its
    device until it is no longer used); a later mount reuses such a device.
    """

    state, detail = mount_state(image_file, mount_point)
    if state == "absent":
        return []
    if state not in ("kernel", "fuse"):
        raise ImageError(
            f"{mount_point} has {detail}; refusing to treat {image_file} as unmounted there"
        )
    backend = state
    routed = _via_user_manager(image_file.parent)
    residual: list[str] = []
    if backend == "kernel":
        argv = ["umount", str(image_file), str(mount_point)]
        if image_file in DISCARD_STALE_COPIES:
            argv.append("--discard-stale-copies")
        _privileged(*argv, anchor=image_file.parent)
        _forget_host_table()
        if not is_mounted(image_file, mount_point):
            if routed or still_attached(image_file):
                # A copy of this mount in a box or an unrelated namespace
                # outlives the host unmount and keeps the loop device bound;
                # detach it.
                release_mount_point_copies(mount_point, image_file.parent)
                _wait_until(lambda: not still_attached(image_file), 0 if routed else 2)
            residual = [item.label for item in still_attached(image_file)]
    else:
        # The kernel detaches the mount before the FUSE server has written its
        # cached metadata back to the image. Mounting the image again while the
        # old server is still flushing loses those writes (a file deleted just
        # before unmount reappears), so wait for the servers found now to exit.
        servers = _fuse_servers(image_file)
        subprocess.run(["fstrim", str(mount_point)], capture_output=True, check=False)
        _run_on_host([shutil.which("fusermount") or "fusermount", "-u", str(mount_point)], anchor=image_file.parent)
        _wait_until(lambda: not is_mounted(image_file, mount_point), 10)
        # A copy of the mount in another namespace (a box's, or an unrelated one)
        # keeps the file system, and so its server, alive: detach such copies
        # when the servers do not exit promptly (at once from a coordinator box).
        if not _wait_for_exit(servers, 0 if routed else 10):
            _forget_host_table()
            if not is_mounted(image_file, mount_point):
                release_mount_point_copies(mount_point, image_file.parent)
            if not _wait_for_exit(servers, 120):
                raise ImageError(f"fuse2fs for {image_file} did not exit after unmount")
    _forget_host_table()
    if is_mounted(image_file, mount_point):
        raise ImageError(f"{mount_point} is still mounted after unmount")
    return residual


def _prepare_fresh_root(image_file: Path, mount_point: Path, anchor: Path | None = None) -> None:
    lost = mount_point / "lost+found"
    if lost.is_dir() and not lost.is_symlink():
        try:
            lost.rmdir()
        except OSError:
            # lost+found belongs to root on a kernel mount. It is harmless
            # residue, but a nested checkout or `git worktree add` needs an
            # empty root, so remove it with the same privilege that mounted it.
            _privileged("rmdir-lost-found", str(image_file), str(mount_point), anchor=anchor)


def provision(
    control: Path,
    slot_type: str,
    slot: str,
    location: Path,
    settings: ImageSettings,
) -> SlotImage:
    """Create both images and mount the slot image at ``location``.

    ``location`` must not exist yet; it is created as the (empty) mount point.
    The caller is responsible for lifecycle journaling; a partially provisioned
    image directory is recognized by its ``provisioning`` phase and completed or
    destroyed by :func:`ensure_mounted` and :func:`destroy`.
    """

    directory = image_directory(control, slot_type, slot)
    if directory.exists() or directory.is_symlink():
        raise ImageError(f"slot image directory already exists: {directory}")
    backend = resolve_backend(settings.backend)
    directory.mkdir(parents=True, mode=0o700)
    image = SlotImage(
        directory=directory,
        slot=slot,
        slot_type=slot_type,
        location=Path(os.path.abspath(location)),
        ceiling_bytes=settings.ceiling_bytes,
        state_ceiling_bytes=settings.state_ceiling_bytes,
        backend=backend,
        created_at=_now(),
        phase="provisioning",
    )
    _write_record(image)
    make_image_file(image.slot_image, image.ceiling_bytes)
    make_image_file(image.state_image, image.state_ceiling_bytes)
    image.state_mount.mkdir(mode=0o700)
    location.mkdir(mode=0o755)
    mount(image.slot_image, image.location, backend)
    _prepare_fresh_root(image.slot_image, image.location, image.directory)
    mount(image.state_image, image.state_mount, backend)
    _prepare_fresh_root(image.state_image, image.state_mount, image.directory)
    for name in STATE_SUBDIRECTORIES:
        (image.state_mount / name).mkdir(mode=0o700, exist_ok=True)
    ready = dataclasses.replace(image, phase="ready")
    _write_record(ready)
    return ready


def ensure_mounted(image: SlotImage, *, remount_discarded: bool = False) -> list[str]:
    """Mount whichever of a slot's images is not mounted; return what was done.

    An image in :data:`DISCARD_PHASE` that is not mounted refuses instead: a
    mount would reuse a loop device that may hold the stale file system. With
    ``remount_discarded`` (``image mount --coordinator-authorized``) it is
    mounted again once no loop device holds either of its images, only from
    the image files (``fresh``), and returns to ``ready``.
    """

    actions: list[str] = []
    if image.phase == DISCARD_PHASE:
        pending = [
            (image_file, mount_point)
            for image_file, mount_point in (
                (image.slot_image, image.location),
                (image.state_image, image.state_mount),
            )
            if not is_mounted(image_file, mount_point)
        ]
        if not pending:
            return actions
        held = [
            item.label for image_file, _point in pending for item in still_attached(image_file)
        ]
        if held or not remount_discarded:
            raise ImageError(discard_hold_message(image, held))
        for image_file, mount_point in pending:
            if not image_file.exists():
                raise ImageError(f"slot image file is missing: {image_file}")
            if not mount_point.exists():
                mount_point.mkdir(mode=0o755 if mount_point == image.location else 0o700)
            mount(image_file, mount_point, image.backend, fresh=True)
            actions.append(
                f"mounted {image_file.name} at {mount_point} from the image file, which a "
                "stale-copy discard had left unmounted; a stale copy released since may have "
                "written its old metadata into it, so check its content before relying on it"
            )
        _write_record(dataclasses.replace(image, phase="ready"))
        return actions
    if image.phase != "ready":
        return actions
    for image_file, mount_point in (
        (image.slot_image, image.location),
        (image.state_image, image.state_mount),
    ):
        if is_mounted(image_file, mount_point):
            continue
        if not image_file.exists():
            raise ImageError(f"slot image file is missing: {image_file}")
        if not mount_point.exists():
            mount_point.mkdir(mode=0o755 if mount_point == image.location else 0o700)
        mount(image_file, mount_point, image.backend)
        reused = _REUSED_DEVICES.get(image_file)
        actions.append(
            f"mounted {image_file.name} at {mount_point}"
            + (
                ""
                if reused is None
                else f" (reusing {reused}, which a copy in another mount namespace still holds)"
            )
        )
    return actions


def ensure_all_mounted(
    control: Path, *, remount_discarded: bool = False
) -> tuple[list[str], list[str]]:
    """Mount every image-backed slot that is not mounted.

    Returns ``(actions, failures)``. One slot that cannot be mounted does not
    stop the others: each failure is reported, naming its slot, and the rest
    are still mounted. ``remount_discarded`` is passed to :func:`ensure_mounted`.
    """

    actions: list[str] = []
    failures: list[str] = []
    for image in all_images(control):
        try:
            actions.extend(ensure_mounted(image, remount_discarded=remount_discarded))
        except (ImageError, OSError) as exc:
            failures.append(f"{image.slot_type}/{image.slot}: {exc}")
    return actions, failures


def relocate(image: SlotImage, destination: Path) -> SlotImage:
    """Move a mounted slot image from its location to ``destination``.

    A mount point cannot be renamed, so this unmounts, renames the now-empty
    directory, records the new location, and mounts again. The unmount fails
    while anything still uses the file system, which is the correct refusal.

    While the image's stale copy is being discarded (:data:`DISCARD_STALE_COPIES`,
    or the image is already in :data:`DISCARD_PHASE`), its record enters that
    phase before the unmount. If any loop device still holds the image after
    the unmount and the release of the copies (a stale copy that stayed
    attached), this refuses before the rename: the image stays unmounted where
    it was, in that phase, because mounting it would reuse that device, and so
    mount the stale file system. Otherwise it is mounted again from the image
    file only (``fresh``) and returns to ``ready``. An image already in that
    phase is relocated only from a mount of its own.
    """

    source = image.location
    destination = Path(os.path.abspath(destination))
    if destination.exists() or destination.is_symlink():
        raise ImageError(f"relocation destination already exists: {destination}")
    discarding = image.slot_image in DISCARD_STALE_COPIES or image.phase == DISCARD_PHASE
    if discarding:
        if image.phase == DISCARD_PHASE and not is_mounted(image.slot_image, source):
            raise ImageError(
                discard_hold_message(
                    image, [item.label for item in still_attached(image.slot_image)]
                )
            )
        image = dataclasses.replace(image, phase=DISCARD_PHASE)
        _write_record(image)
    unmount(image.slot_image, source)
    if discarding:
        # A released loop device is detached asynchronously; give it a moment
        # (unmount waits only outside a coordinator box) before judging.
        _wait_until(lambda: not still_attached(image.slot_image), 5)
        held = [item.label for item in still_attached(image.slot_image)]
        if held:
            raise ImageError(discard_hold_message(image, held))
    if any(source.iterdir()):
        raise ImageError(f"slot mount point {source} is not empty after unmount")
    moved = dataclasses.replace(image, location=destination)
    _write_record(moved)
    rename_mount_point(source, destination, image.directory)
    if not discarding:
        mount(moved.slot_image, destination, image.backend)
        return moved
    mount(moved.slot_image, destination, image.backend, fresh=True)
    moved = dataclasses.replace(moved, phase="ready")
    _write_record(moved)
    return moved


def root_entries(image: SlotImage) -> list[str]:
    """Names at the root of a slot's mounted image."""

    ensure_mounted(image)
    return sorted(entry.name for entry in image.location.iterdir())


def destroy(image: SlotImage, *, allow_content: bool = False) -> None:
    """Unmount and delete a slot's images, then remove its mount point.

    Refuses while the slot image still holds anything but mkfs residue, unless
    ``allow_content`` is set by a caller that has already salvaged it.

    While the image's stale copy is being discarded, its record enters
    :data:`DISCARD_PHASE` after that check and before anything is unmounted,
    so a command interrupted between the unmount and the deletion leaves an
    image that no later command mounts through a device the stale copy may
    still hold. An image already in that phase is checked the same way while
    this host mounts it; unmounted, it goes straight to the unmount and the
    deletion, since nothing may mount it to look.
    """

    entries: list[str] | None = None
    if image.phase == "ready":
        ensure_mounted(image)
        entries = root_entries(image)
    elif image.phase == DISCARD_PHASE and is_mounted(image.slot_image, image.location):
        entries = sorted(entry.name for entry in image.location.iterdir())
    if entries is not None:
        leftover = [name for name in entries if name not in REPRESENTATION_RESIDUE]
        if leftover and not allow_content:
            raise ImageError(
                f"slot image at {image.location} is not empty: {', '.join(leftover[:5])}"
            )
        if image.phase == "ready" and image.slot_image in DISCARD_STALE_COPIES:
            image = dataclasses.replace(image, phase=DISCARD_PHASE)
            _write_record(image)
    unmount(image.state_image, image.state_mount)
    unmount(image.slot_image, image.location)
    if image.location.exists() and not image.location.is_symlink():
        if any(image.location.iterdir()):
            raise ImageError(f"slot mount point {image.location} has content on the host side")
        remove_mount_point(image.location, image.directory)
    for path in (image.slot_image, image.state_image, image.record_path):
        try:
            path.unlink()
        except FileNotFoundError:
            pass
    tmp_record = image.directory / (RECORD_NAME + ".tmp")
    if tmp_record.exists():
        tmp_record.unlink()
    if image.state_mount.exists():
        remove_mount_point(image.state_mount, image.directory)
    image.directory.rmdir()


def usage(image: SlotImage) -> dict[str, object]:
    """Report how much of the host each image costs and how full it is inside."""

    report: dict[str, object] = {
        "name": image.slot,
        "slot_type": image.slot_type,
        "location": str(image.location),
        "phase": image.phase,
    }
    table = mount_table()
    for label, image_file, mount_point in (
        ("slot", image.slot_image, image.location),
        ("state", image.state_image, image.state_mount),
    ):
        entry: dict[str, object] = {"image": str(image_file)}
        try:
            stat = os.stat(image_file)
            entry["ceiling_bytes"] = stat.st_size
            entry["host_allocated_bytes"] = stat.st_blocks * 512
        except OSError as exc:
            entry["error"] = str(exc)
        backend = mounted_backend(image_file, mount_point, table)
        if backend is not None:
            entry["mounted"] = True
            entry["backend"] = backend
            vfs = os.statvfs(mount_point)
            entry["used_bytes"] = (vfs.f_blocks - vfs.f_bfree) * vfs.f_frsize
            entry["free_bytes"] = vfs.f_bavail * vfs.f_frsize
            entry["used_inodes"] = vfs.f_files - vfs.f_ffree
        else:
            entry["mounted"] = False
        report[label] = entry
    return report


def trim(image: SlotImage) -> list[str]:
    """Return freed blocks to the host now (FUSE mounts have no online discard)."""

    done: list[str] = []
    for image_file, mount_point in (
        (image.slot_image, image.location),
        (image.state_image, image.state_mount),
    ):
        backend = mounted_backend(image_file, mount_point)
        if backend is None:
            continue
        if backend == "kernel":
            argv = _host_argv(
                [*_privileged_argv(), "trim", str(image_file), str(mount_point)], image.directory
            )
        else:
            argv = ["fstrim", str(mount_point)]
        result = subprocess.run(argv, capture_output=True, text=True, check=False)
        done.append(f"{mount_point}: {'trimmed' if result.returncode == 0 else result.stderr.strip()}")
    return done


def grow(image: SlotImage, new_ceiling_bytes: int) -> SlotImage:
    """Raise the slot image's ceiling (a larger sparse size), online if possible."""

    current = os.stat(image.slot_image).st_size
    if new_ceiling_bytes <= current:
        raise ImageError(
            f"new ceiling {new_ceiling_bytes} must exceed the current {current} bytes; "
            "shrinking is not supported"
        )
    backend = mounted_backend(image.slot_image, image.location)
    if image.backend == "kernel" or backend == "kernel":
        # A kernel image grows online only. Mounting it first reuses any device
        # that still holds it, so the file system is never resized offline
        # under a superblock some other mount namespace keeps open.
        if backend != "kernel":
            ensure_mounted(image)
        os.truncate(image.slot_image, new_ceiling_bytes)
        _privileged(
            "grow", str(image.slot_image), str(image.location), anchor=image.directory, timeout=600
        )
    else:
        if backend == "fuse":
            unmount(image.slot_image, image.location)
        _refuse_offline_tools_while_attached(image.slot_image)
        os.truncate(image.slot_image, new_ceiling_bytes)
        _run(["e2fsck", "-p", "-f", str(image.slot_image)], timeout=600)
        _run(["resize2fs", str(image.slot_image)], timeout=600)
    grown = dataclasses.replace(image, ceiling_bytes=new_ceiling_bytes)
    _write_record(grown)
    ensure_mounted(grown)
    return grown


def _refuse_offline_tools_while_attached(image_file: Path) -> None:
    """Refuse e2fsck or resize2fs on an image file any loop device may still hold.

    Those tools check only this namespace's mounts, by path. A copy of a mount
    in another namespace keeps a superblock open on the image; changing the
    file under it, and later letting that superblock write itself back, would
    corrupt the image.
    """

    found = image_attachments(image_file)
    if found.devices:
        raise ImageError(
            f"{image_file} is still attached to {', '.join(item.label for item in found.devices)} "
            "(a mount, possibly in another mount namespace, holds it); refusing to change it "
            "offline. Release that mount first"
        )
    if found.unidentified:
        raise ImageError(
            f"cannot prove that no loop device holds {image_file}: "
            f"{', '.join(item.label for item in found.unidentified)} cannot be identified; "
            "refusing to change it offline"
        )


def discard_files(image: SlotImage) -> None:
    """Unmount and delete a slot's images without inspecting or moving content.

    Only for callers that have already copied the slot's content elsewhere (the
    convert-to-worktree path). The mount point itself is left to the caller.
    """

    unmount(image.state_image, image.state_mount)
    unmount(image.slot_image, image.location)
    for path in (image.slot_image, image.state_image, image.record_path):
        try:
            path.unlink()
        except FileNotFoundError:
            pass
    if image.state_mount.exists():
        remove_mount_point(image.state_mount, image.directory)
    image.directory.rmdir()


def tree_listing(root: Path) -> list[str]:
    """A canonical listing used to prove two trees hold the same content.

    Modification times are compared at one-second resolution: fuse2fs stores
    whole seconds, and the copy is proven by path, type, size, and mode first.
    """

    result = subprocess.run(
        ["find", str(root), "-mindepth", "1", "-printf", r"%P\t%y\t%s\t%m\t%T@\t%l\n"],
        capture_output=True,
        text=True,
        errors="surrogateescape",
        check=False,
    )
    if result.returncode != 0:
        raise ImageError(f"cannot list {root}: {result.stderr.strip()[-300:]}")
    rows: list[str] = []
    for row in result.stdout.splitlines():
        fields = row.split("\t")
        if len(fields) == 6:
            if fields[1] == "d":
                fields[2] = "-"  # directory sizes differ between file systems
            fields[4] = fields[4].split(".", 1)[0]
        rows.append("\t".join(fields))
    rows.sort()
    return rows


def _restore_metadata(source: Path, destination: Path) -> None:
    """Reapply modes and times from ``source``; some FUSE servers drop them on copy."""

    for directory, names, files in os.walk(source, topdown=False):
        relative = os.path.relpath(directory, source)
        for name in (*files, *names, "."):
            if name == "." and relative == ".":
                continue
            origin = os.path.normpath(os.path.join(directory, name))
            target = os.path.normpath(os.path.join(destination, relative, name))
            info = os.lstat(origin)
            if not _stat.S_ISLNK(info.st_mode):
                os.chmod(target, _stat.S_IMODE(info.st_mode))
            os.utime(target, ns=(info.st_atime_ns, info.st_mtime_ns), follow_symlinks=False)


def copy_tree(source: Path, destination: Path) -> None:
    """Copy the contents of ``source`` into the existing ``destination``; verify."""

    _run(["cp", "-a", "--reflink=auto", f"{source}/.", f"{destination}/"], timeout=24 * 3600)
    _restore_metadata(source, destination)
    before = [row for row in tree_listing(source) if row.split("\t", 1)[0] not in REPRESENTATION_RESIDUE]
    after = [row for row in tree_listing(destination) if row.split("\t", 1)[0] not in REPRESENTATION_RESIDUE]
    if before != after:
        difference = sorted(set(before) ^ set(after))[:5]
        raise ImageError(f"copy of {source} does not match the original: {difference}")
