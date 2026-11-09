"""One sparse btrfs image holding a project's whole slots tree.

Opt-in via the ``worktrees_image`` configuration section. Two layouts hold
the slots in one sparse btrfs image file that lives outside the mount, on
the host file system. In the default bind layout the image mounts beside
the control directory and every slot subvolume is bind-mounted at its slot
path. With ``mount_at: worktrees_dir`` the project's slots directory
(``worktrees_dir``) is itself the mount point and slot subvolumes are the
image's top-level entries, with no bind mounts or bind records. Every slot
lives inside the image either way, so guest metadata (millions of
small build files) stays out of the host file system, slots can reflink
and snapshot between each other, and per-slot qgroup quotas apply inside
the guest. The registry and journals stay outside the image, beside the
slots directory, so an unmounted or corrupt guest can still be diagnosed.

Mounting is kernel-only: ``sudo -n mount -o loop``. There is no
unprivileged fallback for btrfs (no fuse2fs equivalent exists); a host
without passwordless sudo for mount/loop cannot enable this feature and
keeps plain directories or per-slot ext4 images (``slotimage.py``).

Nothing here changes lifecycle policy. The safety rule is narrow and
hard: when the feature is configured, slot-writing commands refuse
unless the slots directory verifiably *is* the configured image mounted
as btrfs. An unmounted, empty mount point is never written into, so
slots can never be created silently on the host underneath the mount.
"""

from __future__ import annotations

import contextlib
import dataclasses
import fcntl
import json
import os
import subprocess
import uuid
from collections.abc import Callable, Iterator, Sequence
from pathlib import Path

from wrkslots import slotimage

SCHEMA = "wrkslots-worktrees-image/v1"
MIN_SIZE_BYTES = 1024**3
FILESYSTEMS = ("btrfs",)
MOUNTS = ("kernel", "auto")

#: Where the image mounts: beside the control directory with a bind per
#: slot (``binds``, the default), or at the slots directory itself with
#: slot subvolumes as its top-level entries (``worktrees_dir``).
MOUNT_AT_BINDS = "binds"
MOUNT_AT_WORKTREES_DIR = "worktrees_dir"
MOUNT_ATS = (MOUNT_AT_BINDS, MOUNT_AT_WORKTREES_DIR)

#: Quota accounting for a new image: btrfs simple quotas (the default where
#: supported) or full qgroups. See ``apply_quota_mode``.
QUOTA_MODES = ("simple", "full")

#: Name of the validation-slots directory inside the image when the image
#: mounts at the slots directory: ``<worktrees_dir>/validate/<slot>``.
VALIDATE_SUBDIR = "validate"

#: Mount classification states (see ``classify_mount``).
MOUNTED = "mounted"
NOT_MOUNTED = "not-mounted"
WRONG_MOUNT = "wrong-mount"
NOT_EMPTY_UNMOUNTED = "not-empty-unmounted"
MISSING = "missing"


class WorktreesImageError(RuntimeError):
    """A worktrees-image operation could not be completed safely."""


@dataclasses.dataclass(frozen=True)
class WorktreesImageSettings:
    """Configuration for the project worktrees image.

    ``machines`` scopes the whole section to named hosts: the section
    usually lives in a tracked configuration file read on every host
    that checks out the project, while the image exists on one. None
    means the section applies on every host.
    """

    relative_path: str
    size_bytes: int
    filesystem: str = "btrfs"
    mount: str = "kernel"
    slot_quota_bytes: int | None = None
    machines: tuple[str, ...] | None = None
    mount_at: str = MOUNT_AT_BINDS
    quota_mode: str = "simple"

    def applies_on(self, machine: str) -> bool:
        """Whether this section applies on the host named ``machine``."""

        return self.machines is None or machine in self.machines

    def image_file(self, root: Path) -> Path:
        """The backing image file for a project rooted at ``root``."""

        return root / self.relative_path

    @property
    def direct(self) -> bool:
        """Whether the image mounts at the slots directory itself."""

        return self.mount_at == MOUNT_AT_WORKTREES_DIR

    def mount_point(self, control: Path, worktrees: Path) -> Path:
        """Where this project's image mounts under the configured layout."""

        return worktrees if self.direct else image_mount_dir(control)


#: The comment written above each ``worktrees_image`` key in a literate
#: configuration.
WORKTREES_IMAGE_DOCS: dict[str, str] = {
    "path": (
        "Project-relative backing file for the one sparse btrfs image holding every slot. "
        "It lives outside the mount, on the host file system."
    ),
    "size_bytes": (
        "Apparent size of the worktrees image in bytes. The file is sparse: an upper "
        "bound, not a reservation. Grow it later with `wrkslots worktrees-image grow`."
    ),
    "fs": "File system inside the worktrees image. Only btrfs is supported.",
    "mount": (
        "How the worktrees image is mounted: kernel (sudo -n mount -o loop) or auto "
        "(the same; btrfs has no unprivileged FUSE fallback). fuse is refused."
    ),
    "slot_quota_bytes": (
        "Optional per-slot qgroup limit in bytes (referenced, post-compression). "
        "Each new slot becomes a btrfs subvolume inside the image."
    ),
    "machines": (
        "Optional list of machine names (the configuration's machine identity) "
        "the worktrees image applies on. A host not listed behaves exactly as "
        "if the section were absent, so the section is safe to commit: the "
        "image exists on the named hosts only. Absent means every host."
    ),
    "mount_at": (
        "Where the image mounts: binds (default; mounted beside the control files, "
        "one bind mount per slot) or worktrees_dir (mounted at the slots directory, "
        "slot subvolumes as its top-level entries, no binds). Change it only with "
        "`wrkslots worktrees-image migrate-layout`."
    ),
    "quota_mode": (
        "Quota accounting for a new image: simple (default; btrfs simple quotas "
        "where the kernel and btrfs-progs support them, else full qgroups) or full. "
        "Mounting never changes an existing image's mode; status reports the active one."
    ),
}


def settings_from_obj(value: object) -> WorktreesImageSettings:
    """Validate one ``worktrees_image`` section value (already a mapping)."""

    if not isinstance(value, dict):
        raise WorktreesImageError("configuration.worktrees_image must be a mapping")
    unknown = sorted(
        set(value)
        - {
            "path",
            "size_bytes",
            "fs",
            "mount",
            "slot_quota_bytes",
            "machines",
            "mount_at",
            "quota_mode",
        }
    )
    if unknown:
        raise WorktreesImageError(
            f"configuration.worktrees_image has unknown keys: {', '.join(unknown)}"
        )
    raw_path = value.get("path")
    if not isinstance(raw_path, str) or not raw_path.strip():
        raise WorktreesImageError("configuration.worktrees_image.path must be a non-empty string")
    candidate = Path(raw_path)
    if candidate.is_absolute() or ".." in candidate.parts or not candidate.parts:
        raise WorktreesImageError(
            "configuration.worktrees_image.path must be a project-relative path without '..'"
        )
    size = value.get("size_bytes")
    if not isinstance(size, int) or isinstance(size, bool) or size < MIN_SIZE_BYTES:
        raise WorktreesImageError(
            f"configuration.worktrees_image.size_bytes must be an integer of at least {MIN_SIZE_BYTES}"
        )
    filesystem = value.get("fs", "btrfs")
    if filesystem not in FILESYSTEMS:
        raise WorktreesImageError(
            f"configuration.worktrees_image.fs must be one of {', '.join(FILESYSTEMS)}, not {filesystem!r}"
        )
    mount = value.get("mount", "kernel")
    if mount == "fuse":
        raise WorktreesImageError(
            "configuration.worktrees_image.mount 'fuse' is unavailable: btrfs has no "
            "unprivileged FUSE driver (no fuse2fs equivalent); use 'kernel' with "
            "passwordless sudo, or do not enable worktrees_image on this host"
        )
    if mount not in MOUNTS:
        raise WorktreesImageError(
            f"configuration.worktrees_image.mount must be one of {', '.join(MOUNTS)}, not {mount!r}"
        )
    quota = value.get("slot_quota_bytes")
    if quota is not None and (not isinstance(quota, int) or isinstance(quota, bool) or quota < 1):
        raise WorktreesImageError("configuration.worktrees_image.slot_quota_bytes must be a positive integer")
    machines = value.get("machines")
    if machines is not None:
        # Validated on every host, named or not: a malformed scope must
        # never silently read as "applies everywhere" or "nowhere".
        if (
            not isinstance(machines, list)
            or not machines
            or any(not isinstance(name, str) or not name.strip() for name in machines)
            or len(set(machines)) != len(machines)
        ):
            raise WorktreesImageError(
                "configuration.worktrees_image.machines must be a non-empty list of "
                "unique non-empty machine names"
            )
    mount_at = value.get("mount_at", MOUNT_AT_BINDS)
    if mount_at not in MOUNT_ATS:
        raise WorktreesImageError(
            f"configuration.worktrees_image.mount_at must be one of {', '.join(MOUNT_ATS)}, not {mount_at!r}"
        )
    quota_mode = value.get("quota_mode", "simple")
    if quota_mode not in QUOTA_MODES:
        raise WorktreesImageError(
            f"configuration.worktrees_image.quota_mode must be one of {', '.join(QUOTA_MODES)}, not {quota_mode!r}"
        )
    return WorktreesImageSettings(
        relative_path=candidate.as_posix(),
        size_bytes=size,
        filesystem=str(filesystem),
        mount=str(mount),
        slot_quota_bytes=quota,
        machines=tuple(machines) if machines is not None else None,
        mount_at=str(mount_at),
        quota_mode=str(quota_mode),
    )


def classify_mount(
    slots_dir: Path,
    entries: Sequence[slotimage.MountEntry],
    *,
    slots_dir_exists: bool,
    slots_dir_empty: bool,
) -> str:
    """Classify the slots directory's mount state from a mount table.

    Pure: callers supply the table (``slotimage.mount_table()`` in
    production, fixtures in tests) and the directory facts, so every
    state in the design's table is testable without a real mount.
    """

    if not slots_dir_exists:
        return MISSING
    top = None
    for entry in entries:
        if entry.mount_point == slots_dir:
            top = entry  # last match wins: the topmost mount at the path
    if top is None:
        return NOT_MOUNTED if slots_dir_empty else NOT_EMPTY_UNMOUNTED
    if top.fstype != "btrfs":
        return WRONG_MOUNT
    return MOUNTED


def current_state(root: Path, worktrees: Path, settings: WorktreesImageSettings) -> str:
    """Classify the live image mount (reads the real mount table).

    The mount classified is the configured one: beside the control
    directory in the bind layout, the slots directory itself when
    ``settings.direct``. ``root`` is kept for call-site stability.
    """

    mount_dir = worktrees if settings.direct else worktrees.parent / IMAGE_MOUNT_NAME
    exists = mount_dir.is_dir()
    empty = True
    if exists:
        try:
            empty = next(mount_dir.iterdir(), None) is None
        except OSError:
            empty = False
    return classify_mount(
        mount_dir, slotimage.mount_table(), slots_dir_exists=exists, slots_dir_empty=empty
    )


def ensure_image_mounted(
    root: Path,
    control: Path,
    settings: WorktreesImageSettings,
    slots_dir: Path,
) -> None:
    """Mount the image at its configured mount point if it is not mounted."""

    if settings.direct:
        mount_image(settings.image_file(root), slots_dir, settings)
        (slots_dir / VALIDATE_SUBDIR).mkdir(exist_ok=True)
        return
    mount_dir = image_mount_dir(control)
    mount_dir.mkdir(parents=True, exist_ok=True)
    state = classify_mount(
        mount_dir,
        slotimage.mount_table(),
        slots_dir_exists=True,
        slots_dir_empty=next(mount_dir.iterdir(), None) is None,
    )
    if state == MOUNTED:
        return
    _run(
        ["mount", "-o", "loop,compress=zstd:3,noatime", str(settings.image_file(root)), str(mount_dir)],
        sudo=True,
    )
    apply_quota_mode(mount_dir, settings.image_file(root), settings.quota_mode)


def _run(argv: list[str], *, sudo: bool = False) -> subprocess.CompletedProcess[str]:
    command = (["sudo", "-n"] if sudo else []) + argv
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise WorktreesImageError(f"{' '.join(command)} failed: {detail}")
    return result


def _quotas_enabled(mount_point: Path) -> bool:
    """Whether quota accounting is on for the file system at ``mount_point``."""

    try:
        _run(["btrfs", "qgroup", "show", str(mount_point)], sudo=True)
    except WorktreesImageError:
        return False
    return True


def _simple_quota_flag(image_file: Path) -> bool:
    """Whether the image superblock carries the simple-quota feature flag.

    The flag is sticky: once ``quota enable --simple`` sets it, ``quota
    disable`` keeps it, and a later plain ``quota enable`` runs simple
    accounting again. So flag plus quotas-enabled is the active mode.
    """

    try:
        output = _run(["btrfs", "inspect-internal", "dump-super", str(image_file)]).stdout
    except WorktreesImageError:
        return False
    return "SIMPLE_QUOTA" in output


def quota_mode_at(mount_point: Path, image_file: Path) -> str:
    """The active quota mode at a mounted image: ``none``, ``full`` or ``simple``."""

    if not _quotas_enabled(mount_point):
        return "none"
    return "simple" if _simple_quota_flag(image_file) else "full"


def apply_quota_mode(mount_point: Path, image_file: Path, quota_mode: str) -> None:
    """Enable quotas after a fresh mount, never flipping an existing mode.

    A file system that already accounts (either mode) only gets the plain
    idempotent ``quota enable`` every mount uses today; verified on
    kernel 7.1 that it never turns simple quotas back into full qgroups.
    A new file system gets ``--simple`` when configured and supported,
    falling back to a plain enable (full qgroups) when the kernel or
    btrfs-progs refuse it; ``quota_mode_at`` then reports the truth.
    """

    if _quotas_enabled(mount_point):
        _run(["btrfs", "quota", "enable", str(mount_point)], sudo=True)
        return
    if quota_mode == "simple":
        try:
            _run(["btrfs", "quota", "enable", "--simple", str(mount_point)], sudo=True)
            return
        except WorktreesImageError:
            pass
    _run(["btrfs", "quota", "enable", str(mount_point)], sudo=True)


def create_image(image_file: Path, size_bytes: int) -> None:
    """Create the sparse btrfs image. Refuses to overwrite anything.

    Order is load-bearing on a btrfs host (measured in the trial):
    the backing file must be created zero-byte, marked NOCOW
    (``chattr +C``), *then* truncated to size, *then* formatted.
    Setting ``+C`` after ``truncate`` silently leaves the flag unset
    and the host COW-fragments the image per guest write.
    """

    if image_file.exists():
        raise WorktreesImageError(f"worktrees image already exists: {image_file}")
    image_file.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(image_file, os.O_CREAT|os.O_EXCL|os.O_WRONLY, 0o644)
    os.close(descriptor)
    try:
        _run(["chattr", "+C", str(image_file)])
        with open(image_file, "r+b") as handle:
            handle.truncate(size_bytes)
        _run(
            ["mkfs.btrfs", "-f", "-m", "single", "-d", "single", "-L", "wrkslots-worktrees", str(image_file)],
            sudo=True,
        )
    except BaseException:
        image_file.unlink(missing_ok=True)
        raise


def _ensure_mount_root_owner(mount_point: Path) -> None:
    """Give the user the image root so slot create and fence renames work.

    A fresh btrfs root is root-owned; slots are renamed and created at
    the top level in the direct layout, and rename needs write access
    to the parent. The bind layout never needed this (its mount-point
    directories live on the host), so it keeps its root-owned root.
    """

    try:
        owner = mount_point.stat().st_uid
    except OSError as exc:
        raise WorktreesImageError(f"cannot inspect mount root {mount_point}: {exc}") from exc
    if owner != os.getuid():
        _run(["chown", f"{os.getuid()}:{os.getgid()}", str(mount_point)], sudo=True)


def mount_image(
    image_file: Path,
    mount_point: Path,
    settings: WorktreesImageSettings | None = None,
) -> None:
    """Mount the image at ``mount_point`` (idempotent).

    With direct-layout ``settings`` the image root becomes the user's
    and the configured quota mode is applied after a fresh mount.
    """

    direct = settings is not None and settings.direct
    state = classify_mount(
        mount_point,
        slotimage.mount_table(),
        slots_dir_exists=mount_point.is_dir(),
        slots_dir_empty=(next(mount_point.iterdir(), None) is None if mount_point.is_dir() else True),
    )
    if state == MOUNTED:
        if direct:
            _ensure_mount_root_owner(mount_point)
        return
    if state in (WRONG_MOUNT, NOT_EMPTY_UNMOUNTED):
        raise WorktreesImageError(
            f"refusing to mount over {mount_point}: state is {state}; "
            "inspect the directory and any existing mount first"
        )
    if not image_file.is_file():
        raise WorktreesImageError(f"worktrees image file is missing: {image_file}")
    mount_point.mkdir(parents=True, exist_ok=True)
    _run(
        ["mount", "-o", "loop,compress=zstd:3,noatime", str(image_file), str(mount_point)],
        sudo=True,
    )
    apply_quota_mode(
        mount_point, image_file, settings.quota_mode if settings is not None else "simple"
    )
    if direct:
        _ensure_mount_root_owner(mount_point)


def unmount_image(mount_point: Path) -> None:
    """Unmount the worktrees image from the slots directory."""

    _run(["umount", str(mount_point)], sudo=True)


def grow_image(image_file: Path, mount_point: Path, new_size_bytes: int) -> None:
    """Grow online: truncate, refresh loop capacity, resize to max.

    The loop-capacity refresh is required (measured): without
    ``losetup -c`` the device keeps its attach-time size and
    ``btrfs filesystem resize max`` silently keeps the old size.
    """

    current = image_file.stat().st_size
    if new_size_bytes <= current:
        raise WorktreesImageError(
            f"new size {new_size_bytes} does not exceed current apparent size {current}"
        )
    with open(image_file, "r+b") as handle:
        handle.truncate(new_size_bytes)
    holders = _run(["losetup", "-j", str(image_file)], sudo=True).stdout.strip()
    for line in holders.splitlines():
        device = line.split(":", 1)[0].strip()
        if device:
            _run(["losetup", "-c", device], sudo=True)
    _run(["btrfs", "filesystem", "resize", "max", str(mount_point)], sudo=True)


def trim_image(mount_point: Path) -> str:
    """Return freed guest blocks to the host sparse file via fstrim."""

    return _run(["fstrim", "-v", str(mount_point)], sudo=True).stdout.strip()


def is_subvolume(path: Path) -> bool:
    """True when ``path`` is a btrfs subvolume root (inode 256)."""

    try:
        return path.stat().st_ino == 256
    except OSError:
        return False


def create_slot_subvolume(path: Path, quota_bytes: int | None) -> None:
    """Create one slot directory as a btrfs subvolume, with optional quota."""

    _run(["btrfs", "subvolume", "create", str(path)], sudo=True)
    _run(["chown", f"{os.getuid()}:{os.getgid()}", str(path)], sudo=True)
    if quota_bytes is not None:
        _run(["btrfs", "qgroup", "limit", str(quota_bytes), str(path)], sudo=True)


def delete_slot_subvolume(path: Path) -> None:
    """Delete an emptied slot subvolume."""

    _run(["btrfs", "subvolume", "delete", str(path)], sudo=True)


def listing_signature(root: Path) -> list[tuple[str, str, int]]:
    """Sorted (relative path, kind, size) for every migratable entry.

    Special files (FIFOs, sockets, devices — runtime residue in build
    caches) are skipped: they cannot be copied and are never slot
    content, so migration neither copies nor compares them.
    """

    signature: list[tuple[str, str, int]] = []
    for current, dirs, files in os.walk(root):
        for name in dirs + files:
            entry = Path(current) / name
            relative = entry.relative_to(root).as_posix()
            if entry.is_symlink():
                signature.append((relative, "link", len(os.readlink(entry))))
            elif entry.is_dir():
                signature.append((relative, "dir", 0))
            elif entry.is_file():
                signature.append((relative, "file", entry.stat().st_size))
    return sorted(signature)


def _skip_special(directory: str, names: list[str]) -> list[str]:
    """``shutil.copytree`` ignore callback: skip special files."""

    skipped: list[str] = []
    for name in names:
        entry = Path(directory) / name
        if not entry.is_symlink() and not entry.is_dir() and not entry.is_file():
            skipped.append(name)
    return skipped


def process_inside(path: Path) -> bool:
    """Best-effort: any process whose cwd is inside ``path``."""

    target = str(path)
    try:
        proc = Path("/proc")
        entries = list(proc.iterdir())
    except OSError:
        return True
    for entry in entries:
        if not entry.name.isdigit():
            continue
        try:
            cwd = os.readlink(entry / "cwd")
        except OSError:
            continue
        if cwd == target or cwd.startswith(target + "/"):
            return True
    return False


IMAGE_MOUNT_NAME = ".slots-image"
RECORD_NAME = ".worktrees-image.json"


def image_mount_dir(control: Path) -> Path:
    """Where the project image is mounted: beside the control files."""

    return control / IMAGE_MOUNT_NAME


def record_path(control: Path) -> Path:
    """The durable record of slot bind mounts, beside the registry."""

    return control / RECORD_NAME


@dataclasses.dataclass(frozen=True)
class BindRecord:
    """One slot's recorded bind: its subvolume and the mount's identity.

    ``mount_id`` is the ID the bind had in the mounting namespace when it
    was made (field 1 of its mountinfo line). Mount IDs are namespace-local,
    so callers match a live mount by target path, mount root (the subvolume)
    and device, exactly as the slot-image census matches its owned mounts;
    the recorded ID names this exact mount in diagnostics and records.
    """

    subvolume: str
    mount_id: int | None = None
    #: Absolute path the bind currently targets: the canonical slot path, or
    #: its fenced sibling while a removal holds the fence. Relocating the
    #: bind updates this in the same step, so re-establishing binds after a
    #: reboot or config reload never resurrects the canonical mount of a
    #: fenced slot. None in records written before targets were kept; those
    #: bind at the canonical path.
    target: str | None = None


def load_bind_records(control: Path) -> dict[str, BindRecord]:
    """Recorded binds with their mount identity (the earlier string form included).

    Lock-free: writers rename a complete file into place, so a reader
    sees the whole old record or the whole new one, never a mixture.
    """

    path = record_path(control)
    if not path.is_file():
        return {}
    value = json.loads(path.read_text(encoding="utf-8"))
    binds = value.get("binds") if isinstance(value, dict) else None
    if not isinstance(binds, dict):
        raise WorktreesImageError(f"malformed worktrees image record: {path}")
    records: dict[str, BindRecord] = {}
    for slot, item in binds.items():
        if isinstance(item, str):
            records[str(slot)] = BindRecord(subvolume=item)
        elif isinstance(item, dict) and isinstance(item.get("subvolume"), str):
            mount_id = item.get("mount_id")
            target = item.get("target")
            records[str(slot)] = BindRecord(
                subvolume=item["subvolume"],
                mount_id=mount_id if isinstance(mount_id, int) else None,
                target=target if isinstance(target, str) else None,
            )
        else:
            raise WorktreesImageError(f"malformed worktrees image record: {path}")
    return records


def load_binds(control: Path) -> dict[str, str]:
    """Recorded binds: slot name to subvolume name inside the image."""

    return {slot: record.subvolume for slot, record in load_bind_records(control).items()}


#: The flock file serialising every bind-record read-modify-write.
RECORD_LOCK_NAME = ".worktrees-image.json.lock"


@contextlib.contextmanager
def _records_lock(control: Path) -> Iterator[None]:
    """Hold the one flock that serialises bind-record mutations.

    Every writer reads, modifies and writes under this lock, so two
    concurrent wrkslots processes (migrate, create, a config load
    re-establishing binds) can never drop each other's entries. Mounts
    stay outside the lock: no writer holds it across a sudo mount.
    """

    control.mkdir(parents=True, exist_ok=True)
    lock_path = control / RECORD_LOCK_NAME
    descriptor = os.open(
        lock_path, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600
    )
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)


def _write_bind_records(control: Path, records: dict[str, BindRecord]) -> None:
    """Write ``records`` durably; the caller holds the records lock.

    A unique temp name per writer, fsync of the file before the rename
    and of the directory after it: a crash leaves the old record or the
    new one, never a torn file, and never a shared temp to clobber.
    """

    path = record_path(control)
    payload = {
        "binds": {
            slot: {
                "subvolume": record.subvolume,
                "mount_id": record.mount_id,
                "target": record.target,
            }
            for slot, record in sorted(records.items())
        }
    }
    temporary = path.with_name(f"{path.name}.tmp.{os.getpid()}.{uuid.uuid4().hex}")
    descriptor = os.open(
        temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o644
    )
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(json.dumps(payload, indent=2, sort_keys=True) + "\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    directory = os.open(control, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def _update_bind_records(
    control: Path,
    mutate: Callable[[dict[str, BindRecord]], dict[str, BindRecord]],
) -> dict[str, BindRecord]:
    """The only record-mutation path: lock, load, mutate, write if changed."""

    with _records_lock(control):
        before = load_bind_records(control)
        after = mutate(dict(before))
        if after != before:
            _write_bind_records(control, after)
        return after


def save_bind_records(control: Path, records: dict[str, BindRecord]) -> None:
    """Persist the bind records atomically beside the registry."""

    _update_bind_records(control, lambda _current: dict(records))


def save_binds(control: Path, binds: dict[str, str]) -> None:
    """Persist the bind record atomically beside the registry."""

    save_bind_records(
        control, {slot: BindRecord(subvolume=sub) for slot, sub in binds.items()}
    )


def mount_id_at(mount_point: Path, mountinfo: Path = Path("/proc/self/mountinfo")) -> int | None:
    """The mount ID of the topmost mount at ``mount_point``, or None."""

    try:
        text = mountinfo.read_text(encoding="utf-8", errors="surrogateescape")
    except OSError:
        return None
    found: int | None = None
    for line in text.splitlines():
        fields = line.split()
        if len(fields) < 5 or fields[4].replace("\\040", " ") != str(mount_point):
            continue
        try:
            found = int(fields[0])
        except ValueError:
            continue
    return found


def bind_slot(control: Path, mount_dir: Path, slot: str, slot_path: Path) -> None:
    """Bind a slot's image subvolume at its original real path; record it.

    Crash order (mount, then record): a crash between them leaves a
    mounted bind with no record. ``ensure_binds`` ignores it (nothing
    recorded), the mount census counts it as foreign use of the slot
    path until it is unmounted, and a retried bind records it.
    """

    source = mount_dir / slot
    if not source.is_dir():
        raise WorktreesImageError(f"slot subvolume is missing: {source}")
    slot_path.mkdir(parents=True, exist_ok=True)
    _run(["mount", "--bind", str(source), str(slot_path)], sudo=True)
    mount_id = mount_id_at(slot_path)

    def record(records: dict[str, BindRecord]) -> dict[str, BindRecord]:
        records[slot] = BindRecord(
            subvolume=slot, mount_id=mount_id, target=str(slot_path)
        )
        return records

    _update_bind_records(control, record)


def relocate_bind(control: Path, mount_dir: Path, slot: str, source: Path, destination: Path) -> None:
    """Move a slot's bind from ``source`` to ``destination``; record follows.

    A mount point cannot be renamed, so the bind is unmounted, the empty
    mount point renamed, and the subvolume bound again at the destination.
    A failed unmount changes nothing: the slot, its mount and its record all
    stay at ``source``, so the caller refuses and a retry starts clean.

    Crash order (unmount, rename, mount, then record): before the rename
    the record still names ``source`` and ``ensure_binds`` re-binds there,
    healing the move away. After the rename the record still names the
    old target while the fenced destination exists, and the fenced-sibling
    guard in ``ensure_binds`` leaves the slot alone for recovery.
    """

    current = load_bind_records(control).get(slot)
    if current is None:
        raise WorktreesImageError(f"slot {slot} has no recorded worktrees-image bind")
    _run(["umount", str(source)], sudo=True)
    try:
        source.rename(destination)
    except OSError:
        _run(["mount", "--bind", str(mount_dir / current.subvolume), str(source)], sudo=True)
        raise
    _run(["mount", "--bind", str(mount_dir / current.subvolume), str(destination)], sudo=True)
    mount_id = mount_id_at(destination)

    def relocate(records: dict[str, BindRecord]) -> dict[str, BindRecord]:
        fresh = records.get(slot, current)
        records[slot] = BindRecord(
            subvolume=fresh.subvolume, mount_id=mount_id, target=str(destination)
        )
        return records

    _update_bind_records(control, relocate)


def umount_point(path: Path) -> None:
    """Unmount whatever is mounted at ``path``; the bind record is untouched."""

    _run(["umount", str(path)], sudo=True)


def unbind_slot(control: Path, slot: str, slot_path: Path) -> None:
    """Unmount a slot's bind and drop its record (original stays beneath).

    Crash order (unmount, then record): a crash between them leaves a
    record saying bound with nothing mounted and the subvolume intact,
    and the next config load's ``ensure_binds`` re-binds it, so a remove
    interrupted here retries from the bound state. A record that
    outlives its subvolume (deleted outside this order) is skipped by
    ``ensure_binds``, never re-created and never a load failure.
    """

    _run(["umount", str(slot_path)], sudo=True)

    def drop(records: dict[str, BindRecord]) -> dict[str, BindRecord]:
        records.pop(slot, None)
        return records

    _update_bind_records(control, drop)


def ensure_binds(
    control: Path,
    mount_dir: Path,
    slots_dir: Path,
    settings: WorktreesImageSettings | None = None,
) -> list[str]:
    """Re-establish the image mount and every recorded bind; return actions.

    Each bind goes back at its recorded target. A slot whose canonical path
    has a fenced sibling is mid-removal: its bind lives at (or is headed to)
    the fenced path, and resurrecting the canonical mount would make every
    later recovery refuse that both paths exist, so it is left alone.

    The direct layout has no binds at all: with direct ``settings`` this
    returns before any record is read or written.
    """

    if settings is not None and settings.direct:
        return []
    actions: list[str] = []
    if not mount_dir.is_dir() or next(mount_dir.iterdir(), None) is None:
        pass
    records = load_bind_records(control)
    entries = slotimage.mount_table()
    mounted_points = {entry.mount_point for entry in entries}
    for slot in sorted(records):
        record = records[slot]
        canonical = slots_dir / slot
        target = Path(record.target) if record.target else canonical
        if target == canonical and any(
            sibling.name.startswith(f".{slot}.fenced.")
            for sibling in slots_dir.glob(f".{slot}.fenced.*")
        ):
            continue
        if target in mounted_points:
            continue
        if not (mount_dir / record.subvolume).is_dir():
            # The record outlived its subvolume (a remove deleted the
            # subvolume outside unbind's crash order). Skip it: do not
            # fail every config load, do not re-create the slot path or
            # subvolume, and do not block the remaining binds.
            continue
        bind_slot(control, mount_dir, slot, target)
        actions.append(f"bound slot {slot} from the worktrees image")
    return actions


def status_report(root: Path, worktrees: Path, settings: WorktreesImageSettings) -> dict[str, object]:
    """Mount state, quota mode, apparent and host-allocated bytes of the image."""
    image_file = settings.image_file(root)
    mount_point = worktrees if settings.direct else worktrees.parent / IMAGE_MOUNT_NAME
    state = current_state(root, worktrees, settings)
    report: dict[str, object] = {
        "image_file": str(image_file),
        "mount_point": str(mount_point),
        "mount_at": settings.mount_at,
        "configured_size_bytes": settings.size_bytes,
        "state": state,
        "image_exists": image_file.is_file(),
        "quota_mode": (
            quota_mode_at(mount_point, image_file) if state == MOUNTED else "unknown"
        ),
        "configured_quota_mode": settings.quota_mode,
        "binds": {} if settings.direct else load_binds(worktrees.parent),
    }
    if image_file.is_file():
        stat = image_file.stat()
        report["apparent_bytes"] = stat.st_size
        report["allocated_bytes"] = stat.st_blocks * 512
    return report
