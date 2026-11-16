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
unless the configured mount point verifiably *is* the configured image
mounted as btrfs. An unmounted, empty mount point is never written into, so
slots can never be created silently on the host underneath the mount.
"""

from __future__ import annotations

import contextlib
import dataclasses
import fcntl
import json
import os
import subprocess
import time
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

#: Name of the validation-slots directory: inside the image it is the
#: top-level ``validate`` entry; in every layout validation slots surface
#: at ``<managed-root>/validate/<slot>`` (see ``validate_external_dir``).
VALIDATE_SUBDIR = "validate"

#: Name of the in-image Eden backing directory: a top-level subvolume of
#: its own holding one directory per backing repo (#1567). It is never a
#: slot (the dot prefix cannot be a slot name) and never removed with one.
EDEN_BACKING_SUBDIR = ".eden-backing"

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
    #: In-image Eden backing repo name (#1567): the single-component
    #: directory name under ``.eden-backing/`` every Eden slot in this
    #: project's image must clone from. None keeps caller-supplied
    #: backing paths (the historical behaviour).
    eden_backing_repo: str | None = None

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
    "eden_backing_repo": (
        "Optional name of the one Eden backing repository inside the image "
        "(<image mount>/.eden-backing/<name>, its own subvolume outside every "
        "slot). When set, create --eden accepts only that name or its resolved "
        "in-image path and refuses anything else, so Eden metadata stays in "
        "the image. Seed it with `wrkslots worktrees-image seed-eden-backing`; "
        "absent keeps caller-supplied backing paths."
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
            "eden_backing_repo",
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
    eden_backing = value.get("eden_backing_repo")
    if eden_backing is not None:
        # Validated on every host like `machines`: one plain directory
        # component, never the validation entry or a traversal.
        if (
            not isinstance(eden_backing, str)
            or not eden_backing
            or eden_backing != eden_backing.strip()
            or "/" in eden_backing
            or eden_backing in {".", "..", VALIDATE_SUBDIR}
            or any(char.isspace() for char in eden_backing)
        ):
            raise WorktreesImageError(
                "configuration.worktrees_image.eden_backing_repo must be one "
                "plain directory name (no path separators, spaces, or 'validate')"
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
        eden_backing_repo=str(eden_backing) if eden_backing is not None else None,
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


def validate_image_dir(slots_dir: Path) -> Path:
    """The validation storage directory inside the mounted image."""

    return slots_dir / VALIDATE_SUBDIR


def eden_backing_root(mount_point: Path) -> Path:
    """The in-image Eden backing root: a top-level entry of the mount (#1567)."""

    return mount_point / EDEN_BACKING_SUBDIR


def eden_backing_repo_path(mount_point: Path, name: str) -> Path:
    """One in-image Eden backing repo inside :func:`eden_backing_root`."""

    return eden_backing_root(mount_point) / name


def ensure_eden_backing_root(mount_point: Path) -> Path:
    """Create the backing root as its own subvolume if absent (no quota).

    Bounded by the image itself; a per-slot quota must never cap the
    store every slot shares. Never removed with a slot: every slot
    deletion path names only that slot's own subvolume or path.
    """

    root = eden_backing_root(mount_point)
    if not root.exists():
        create_slot_subvolume(root, None)
    return root


def validate_external_dir(slots_dir: Path) -> Path:
    """Where validation slots surface, in every layout.

    ``worktrees/validate`` when the slots directory is named ``slots``
    (the managed root is its parent), else inside the slots directory
    itself, where it coincides with the image directory.
    """

    managed = slots_dir.parent if slots_dir.name == "slots" else slots_dir
    return managed / VALIDATE_SUBDIR


def _mounted_at(point: Path) -> bool:
    return any(entry.mount_point == point for entry in slotimage.mount_table())


def ensure_validate_bind(slots_dir: Path) -> None:
    """Bind the image's validate directory at its surface path.

    One extra bind, owned by wrkslots: mounted here and in
    ``mount_image``, unmounted in ``unmount_image`` and reported in
    ``status_report``. When the surface coincides with the storage
    directory there is nothing to bind.
    """

    internal = validate_image_dir(slots_dir)
    internal.mkdir(parents=True, exist_ok=True)
    external = validate_external_dir(slots_dir)
    if external == internal:
        return
    external.mkdir(parents=True, exist_ok=True)
    if _mounted_at(external):
        return
    _bind_private(internal, external)


def unmount_validate_bind(slots_dir: Path) -> None:
    """Unmount the validate bind, if it is mounted."""

    external = validate_external_dir(slots_dir)
    if external == validate_image_dir(slots_dir):
        return
    if _mounted_at(external):
        unmount_bind(external)


def ensure_image_mounted(
    root: Path,
    control: Path,
    settings: WorktreesImageSettings,
    slots_dir: Path,
) -> None:
    """Mount the image at its configured mount point if it is not mounted."""

    if settings.direct:
        mount_image(settings.image_file(root), slots_dir, settings)
        ensure_validate_bind(slots_dir)
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


def _parse_qgroup_referenced(text: str, qgroupid: str) -> int | None:
    """Referenced bytes for one qgroup from raw ``qgroup show`` output."""

    for line in text.splitlines():
        fields = line.split()
        if len(fields) >= 2 and fields[0] == qgroupid:
            try:
                return int(fields[1])
            except ValueError:
                return None
    return None


def qgroup_referenced_bytes(mount_point: Path, subvolume: Path) -> int | None:
    """Referenced bytes of one subvolume, or None when unavailable."""

    try:
        rootid = _run(
            ["btrfs", "inspect-internal", "rootid", str(subvolume)]
        ).stdout.strip()
        output = _run(
            ["btrfs", "qgroup", "show", "-re", "--raw", str(mount_point)],
            sudo=True,
        ).stdout
    except WorktreesImageError:
        return None
    if not rootid.isdigit():
        return None
    return _parse_qgroup_referenced(output, f"0/{rootid}")


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
    # 0600, like the per-slot images: the file holds every slot, so no
    # other local user may read slots out of it.
    descriptor = os.open(image_file, os.O_CREAT|os.O_EXCL|os.O_WRONLY, 0o600)
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
            ensure_validate_bind(mount_point)
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
        ensure_validate_bind(mount_point)


def unmount_image(mount_point: Path) -> None:
    """Unmount the worktrees image from ``mount_point``.

    For the direct layout (``mount_point`` is the slots directory) the
    validate bind at ``validate_external_dir(mount_point)`` comes down
    first; unmounting the image would otherwise leave it dangling.
    """

    unmount_validate_bind(mount_point)
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


def _content_hash(path: Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def listing_signature(root: Path) -> list[tuple[str, str, int, int, str]]:
    """Sorted (path, kind, size, mode, proof) for every migratable entry.

    Proof is the link target for a symlink and the SHA-256 of the
    contents for a file, so the migrate compare catches a same-size
    content difference, a mode change, and a retargeted link — not
    just a different listing. Special files (FIFOs, sockets, devices
    — runtime residue in build caches) are skipped: they cannot be
    copied and are never slot content, so migration neither copies
    nor compares them.
    """

    import stat as stat_module

    signature: list[tuple[str, str, int, int, str]] = []
    for current, dirs, files in os.walk(root):
        for name in dirs + files:
            entry = Path(current) / name
            relative = entry.relative_to(root).as_posix()
            if entry.is_symlink():
                mode = stat_module.S_IMODE(entry.lstat().st_mode)
                signature.append((relative, "link", 0, mode, os.readlink(entry)))
            elif entry.is_dir():
                mode = stat_module.S_IMODE(entry.stat().st_mode)
                signature.append((relative, "dir", 0, mode, ""))
            elif entry.is_file():
                info = entry.stat()
                signature.append(
                    (
                        relative,
                        "file",
                        info.st_size,
                        stat_module.S_IMODE(info.st_mode),
                        _content_hash(entry),
                    )
                )
    return sorted(signature)


def git_state_signature(root: Path) -> list[tuple[str, str, str]]:
    """Sorted (checkout, HEAD, status) for each Git checkout under root.

    A directory holding a ``.git`` entry (directory or worktree
    pointer file) contributes its ``HEAD`` and porcelain status, so
    the migrate compare also proves the copy is the same checkout in
    the same state, not merely the same bytes. A checkout Git cannot
    read contributes its error text, which must then match too.
    """

    states: list[tuple[str, str, str]] = []
    for current, dirs, _files in os.walk(root):
        directory = Path(current)
        if ".git" not in dirs and not (directory / ".git").is_file():
            continue
        relative = directory.relative_to(root).as_posix()
        values: list[str] = []
        for argv in (
            ["rev-parse", "HEAD"],
            ["status", "--porcelain=v1", "--untracked-files=no"],
        ):
            # --no-optional-locks: a copied linked worktree shares its
            # administrative directory with the original, and a status
            # refresh must never write the shared index from here.
            result = subprocess.run(
                ["git", "--no-optional-locks", "-C", str(directory), *argv],
                capture_output=True,
                text=True,
                check=False,
            )
            values.append(
                result.stdout.strip()
                if result.returncode == 0
                else f"unreadable: {(result.stderr or result.stdout).strip()}"
            )
        states.append((relative, values[0], values[1]))
        if ".git" in dirs:
            dirs.remove(".git")
    return sorted(states)


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


#: Bounded unmount retries after privatising (#1733): a holder that is
#: exiting gets a chance to release its copy; a holder that stays
#: is named by ``describe_mount_holders`` instead of retried forever.
UMOUNT_ATTEMPTS = 3

#: Re-checks of the holder scan waited on between unmount attempts
#: (#1743): the wait ends when the holder set changes or vanishes, and
#: is bounded by this count, never by a wall-clock budget. The poll
#: interval below only paces the re-checks; it decides nothing.
UMOUNT_RECHECKS = 5
UMOUNT_RECHECK_SECONDS = 0.05

#: Injectable wait between unmount retries (tests substitute a counter,
#: never a wall clock).
_sleep = time.sleep

#: Injectable process-tree root for the holder scan (fixtures only).
_PROC_ROOT = Path("/proc")


def _top_entry(
    target: Path, entries: Sequence[slotimage.MountEntry]
) -> slotimage.MountEntry | None:
    found: slotimage.MountEntry | None = None
    for entry in entries:
        if entry.mount_point == target:
            found = entry
    return found


def peer_views(
    target: Path, entries: Sequence[slotimage.MountEntry]
) -> list[Path]:
    """Every mount point showing the same bind as ``target``.

    A slot bind under a mirrored tree appears once per mirror root with
    the same device and mount root. All of them share one peer group
    while the bind is shared, so privatising or unmounting only the
    canonical path leaves the mirror copy able to pin the slot (#1733).
    Falls back to ``target`` alone when the table shows no mount there.
    """

    top = _top_entry(target, entries)
    if top is None:
        return [target]
    views = sorted(
        {
            entry.mount_point
            for entry in entries
            if entry.device == top.device and entry.root == top.root
        }
        | {target}
    )
    return views


def make_private(
    target: Path, entries: Sequence[slotimage.MountEntry] | None = None
) -> None:
    """Make the bind at ``target`` (and every peer view) private.

    Private, not slave: nothing outside wrkslots legitimately needs
    mounts to propagate into a slot bind — a sandbox that needs the
    slot contents reads them through the path, and propagation inward
    is exactly how foreign namespaces came to hold pinning copies.
    """

    table = slotimage.mount_table() if entries is None else entries
    for view in peer_views(target, table):
        _run(["mount", "--make-private", str(view)], sudo=True)


def _bind_private(source: Path, target: Path) -> None:
    """Bind ``source`` at ``target`` and privatise every view at once.

    A bind created shared is copied into every private namespace that
    starts while it exists, so the privatise is part of creating the
    bind, never a later fix-up. If it fails the fresh bind comes down
    again rather than staying shared.
    """

    _run(["mount", "--bind", str(source), str(target)], sudo=True)
    try:
        make_private(target)
    except BaseException:
        with contextlib.suppress(WorktreesImageError):
            _run(["umount", str(target)], sudo=True)
        raise


def _unit_for_pid(proc_root: Path, pid: str) -> str | None:
    try:
        text = (proc_root / pid / "cgroup").read_text(
            encoding="utf-8", errors="replace"
        )
    except OSError:
        return None
    for line in text.splitlines():
        for part in line.rsplit("/", 1)[-1].split("/"):
            if part.endswith((".service", ".scope")):
                return part
    return None


@dataclasses.dataclass(frozen=True)
class _MountHolder:
    """One process whose mount namespace still holds a slot bind (#1743)."""

    pid: str
    comm: str | None
    namespace: str | None
    unit: str | None
    start_epoch: float | None

    @property
    def signature(self) -> tuple[str, str | None]:
        return (self.pid, self.namespace)

    def proven_older_than(self, bind_epoch: float | None) -> bool:
        """Whether this holder certainly starts before the slot's bind."""
        return (
            bind_epoch is not None
            and self.start_epoch is not None
            and self.start_epoch <= bind_epoch
        )

    def describe(self) -> str:
        parts = [
            part
            for part in (
                self.comm,
                f"mnt:[{self.namespace}]" if self.namespace else None,
                self.unit,
            )
            if part
        ]
        return f"pid {self.pid} ({', '.join(parts)})" if parts else f"pid {self.pid}"


def _process_start_epoch(proc_root: Path, pid_dir: Path) -> float | None:
    """Start time of one process as an epoch, from its stat and boot time."""
    try:
        text = (pid_dir / "stat").read_text(encoding="utf-8", errors="replace")
        boot = (proc_root / "stat").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    close = text.rfind(")")
    if close < 0:
        return None
    fields = text[close + 1 :].split()
    # fields[0] is state (field 3 overall); start time is field 22.
    if len(fields) <= 19:
        return None
    btime = None
    for line in boot.splitlines():
        if line.startswith("btime "):
            try:
                btime = int(line.split()[1])
            except (IndexError, ValueError):
                return None
            break
    if btime is None:
        return None
    try:
        ticks = int(fields[19])
        hertz = os.sysconf("SC_CLK_TCK")
    except (ValueError, OSError):
        return None
    return btime + ticks / hertz


def _namespace_id(pid_dir: Path) -> str | None:
    """Mount-namespace id of one process, from its ``ns/mnt`` link."""
    link = pid_dir / "ns" / "mnt"
    try:
        raw = os.readlink(link)
    except OSError:
        try:
            raw = link.read_text(encoding="utf-8", errors="replace").strip()
        except OSError:
            return None
    start = raw.find("[")
    end = raw.find("]", start + 1) if start >= 0 else -1
    if start < 0 or end < 0:
        return None
    return raw[start + 1 : end] or None


def scan_mount_holders(
    target: Path,
    proc_root: Path | None = None,
    entries: Sequence[slotimage.MountEntry] | None = None,
) -> list[_MountHolder] | None:
    """Every holder of ``target``; None when the process tree is unreadable.

    Read-only (#1743): a holder is never killed or entered, only named.
    """

    root = _PROC_ROOT if proc_root is None else proc_root
    table = slotimage.mount_table() if entries is None else entries
    top = _top_entry(target, table)
    views = set(peer_views(target, table))
    try:
        pid_dirs = sorted(
            (entry for entry in root.iterdir() if entry.name.isdigit()),
            key=lambda entry: int(entry.name),
        )
    except OSError:
        return None
    holders: list[_MountHolder] = []
    for pid_dir in pid_dirs:
        try:
            theirs = slotimage._parse_mount_table(
                (pid_dir / "mountinfo").read_text(
                    encoding="utf-8", errors="surrogateescape"
                )
            )
        except (slotimage.ImageError, OSError):
            continue
        held = any(
            entry.mount_point in views
            or (
                top is not None
                and entry.device == top.device
                and entry.root == top.root
            )
            for entry in theirs
        )
        if not held:
            continue
        try:
            comm = (pid_dir / "comm").read_text(
                encoding="utf-8", errors="replace"
            ).strip() or None
        except OSError:
            comm = None
        holders.append(
            _MountHolder(
                pid=pid_dir.name,
                comm=comm,
                namespace=_namespace_id(pid_dir),
                unit=_unit_for_pid(root, pid_dir.name),
                start_epoch=_process_start_epoch(root, pid_dir),
            )
        )
    return holders


def describe_mount_holders(
    target: Path,
    proc_root: Path | None = None,
    entries: Sequence[slotimage.MountEntry] | None = None,
) -> str:
    """Name the processes whose mount namespaces still hold ``target``.

    Scans each process's mount table for the bind's identity (device
    and mount root) or any of its peer views, and names the pid, comm,
    namespace id and, when the cgroup names one, its unit (#1743).
    """

    holders = scan_mount_holders(target, proc_root=proc_root, entries=entries)
    if holders is None:
        return "holder scan unavailable (process tree unreadable)"
    if not holders:
        return "no process mount namespace still holds it"
    return ", ".join(holder.describe() for holder in holders)


def _wait_on_holder_recheck(
    target: Path,
    previous: list[_MountHolder] | None,
    *,
    wait: Callable[[float], None],
    proc_root: Path | None,
    entries: Sequence[slotimage.MountEntry],
) -> None:
    """Wait until the holder set changes or vanishes (#1743).

    Bounded by UMOUNT_RECHECKS re-checks, never by a wall-clock budget:
    each re-check rescans the holders, and the wait ends as soon as the
    signature set differs. Holders are only read, never disturbed.
    """

    before = (
        None
        if previous is None
        else tuple(holder.signature for holder in previous)
    )
    for _recheck in range(UMOUNT_RECHECKS):
        wait(UMOUNT_RECHECK_SECONDS)
        current = scan_mount_holders(target, proc_root=proc_root, entries=entries)
        if current is None:
            return
        now = tuple(holder.signature for holder in current)
        if not now or now != before:
            return


def unmount_bind(
    target: Path,
    *,
    attempts: int = UMOUNT_ATTEMPTS,
    sleeper: Callable[[float], None] | None = None,
    proc_root: Path | None = None,
    bind_epoch: float | None = None,
) -> None:
    """Privatise then unmount the bind at ``target`` and its peer views.

    After the fence, privatising first stops the unmount from being
    pinned by copies handed to namespaces that started earlier, and
    detaches the mirror view so it cannot re-pin the canonical path
    (#1733). A busy unmount whose every holder is transient -- younger
    than the bind, or gone on re-check -- retries a bounded number of
    times, waiting on holder re-checks between tries (#1743); a holder
    proven older than the bind, and a still-busy unmount after the
    bound, are named in full instead of retried forever. ``bind_epoch``
    defaults to the bind target's own change time, which the slot's
    creation sets before the bind, so an older holder is proven, never
    guessed. Holders are never killed or entered.
    """

    wait = _sleep if sleeper is None else sleeper
    if bind_epoch is None:
        try:
            bind_epoch = target.stat().st_ctime
        except OSError:
            bind_epoch = None
    last_error: WorktreesImageError | None = None
    for attempt in range(max(1, attempts)):
        try:
            table = slotimage.mount_table()
        except slotimage.ImageError:
            table = []
        views = peer_views(target, table)
        try:
            for view in views:
                _run(["mount", "--make-private", str(view)], sudo=True)
            was_mounted = target in {entry.mount_point for entry in table}
            for view in views:
                # Unmounting one view can take its peers down with it
                # (the parent tree is still shared), so re-read the
                # table and skip a view that is already gone. With no
                # table entry at all (a stubbed table in tests) the
                # target is still unmounted unconditionally.
                try:
                    fresh = slotimage.mount_table()
                except slotimage.ImageError:
                    fresh = table
                mounted = {entry.mount_point for entry in fresh}
                if view in mounted or (view == target and not was_mounted):
                    _run(["umount", str(view)], sudo=True)
            return
        except WorktreesImageError as exc:
            last_error = exc
            if "busy" not in str(exc).lower():
                raise
            if attempt + 1 < max(1, attempts):
                holders_now = scan_mount_holders(
                    target, proc_root=proc_root, entries=table
                )
                if holders_now and all(
                    holder.proven_older_than(bind_epoch) for holder in holders_now
                ):
                    break
                _wait_on_holder_recheck(
                    target, holders_now, wait=wait, proc_root=proc_root, entries=table
                )
    holders = describe_mount_holders(target, proc_root=proc_root)
    raise WorktreesImageError(
        f"umount {target}: target is busy after {max(1, attempts)} attempts; "
        f"still held by {holders}"
    ) from last_error


def removal_journaled_slots(control: Path) -> set[str]:
    """Slots with an outstanding removal journal in ``control``.

    A finish journal names its slot directly; a validate-batch seal
    names each target. An unreadable journal proves nothing either way
    and is left to the journal machinery, as before (#1733).
    """

    slots: set[str] = set()
    patterns = ("FINISH.*.journal", "VALIDATE-BATCH-SEAL.*.journal")
    for pattern in patterns:
        for path in sorted(control.glob(pattern)):
            try:
                raw: object = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, json.JSONDecodeError):
                continue
            payload = raw.get("journal") if isinstance(raw, dict) else None
            record = payload if isinstance(payload, dict) else raw
            if not isinstance(record, dict):
                continue
            slot = record.get("slot")
            if isinstance(slot, str):
                slots.add(slot)
            targets = record.get("targets")
            if isinstance(targets, list):
                for item in targets:
                    if isinstance(item, dict) and isinstance(item.get("slot"), str):
                        slots.add(item["slot"])
                    elif isinstance(item, (list, tuple)) and item and isinstance(item[0], str):
                        slots.add(item[0])
            listed = record.get("slots")
            if isinstance(listed, list):
                slots.update(name for name in listed if isinstance(name, str))
    return slots


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
    _bind_private(source, slot_path)
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
    unmount_bind(source)
    try:
        source.rename(destination)
    except OSError:
        _bind_private(mount_dir / current.subvolume, source)
        raise
    _bind_private(mount_dir / current.subvolume, destination)
    mount_id = mount_id_at(destination)

    def relocate(records: dict[str, BindRecord]) -> dict[str, BindRecord]:
        fresh = records.get(slot, current)
        records[slot] = BindRecord(
            subvolume=fresh.subvolume, mount_id=mount_id, target=str(destination)
        )
        return records

    _update_bind_records(control, relocate)


def forget_bind(control: Path, slot: str) -> None:
    """Drop a slot's bind record only; mounts are the caller's business."""

    def drop(records: dict[str, BindRecord]) -> dict[str, BindRecord]:
        records.pop(slot, None)
        return records

    _update_bind_records(control, drop)


def umount_point(path: Path) -> None:
    """Unmount the bind at ``path``; the bind record is untouched."""

    unmount_bind(path)


def unbind_slot(control: Path, slot: str, slot_path: Path) -> None:
    """Unmount a slot's bind and drop its record (original stays beneath).

    Crash order (unmount, then record): a crash between them leaves a
    record saying bound with nothing mounted and the subvolume intact,
    and the next config load's ``ensure_binds`` re-binds it, so a remove
    interrupted here retries from the bound state. A record that
    outlives its subvolume (deleted outside this order) is skipped by
    ``ensure_binds``, never re-created and never a load failure.
    """

    unmount_bind(slot_path)

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
    later recovery refuse that both paths exist, so it is left alone. A slot
    whose removal is journaled (a finish journal or a validate-batch seal)
    is likewise left alone: re-binding it, shared, is what pinned the
    removal that journaled it (#1733). A bind that is already mounted but
    still shared is made private here, so existing binds migrate without
    an operator step.

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
    journaled = removal_journaled_slots(control)
    for slot in sorted(records):
        record = records[slot]
        canonical = slots_dir / slot
        target = Path(record.target) if record.target else canonical
        if slot in journaled:
            continue
        if target == canonical and any(
            sibling.name.startswith(f".{slot}.fenced.")
            for sibling in slots_dir.glob(f".{slot}.fenced.*")
        ):
            continue
        if target in mounted_points:
            top = _top_entry(target, entries)
            if top is not None and "shared:" in top.propagation:
                make_private(target, entries)
                actions.append(f"made slot {slot} bind private")
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
        "validate_dir": str(validate_external_dir(worktrees)),
        "validate_bind": (
            "mounted"
            if settings.direct and _mounted_at(validate_external_dir(worktrees))
            else "not-mounted"
            if settings.direct
            else "n/a"
        ),
    }
    if image_file.is_file():
        stat = image_file.stat()
        report["apparent_bytes"] = stat.st_size
        report["allocated_bytes"] = stat.st_blocks * 512
    if settings.eden_backing_repo is not None:
        from wrkslots import eden

        backing_path = eden_backing_repo_path(mount_point, settings.eden_backing_repo)
        problem = (
            eden.backing_repo_problem(backing_path) if state == MOUNTED else None
        )
        report["eden_backing"] = {
            "configured_name": settings.eden_backing_repo,
            "path": str(backing_path),
            "present": backing_path.is_dir() if state == MOUNTED else False,
            "repo_ok": problem is None and state == MOUNTED,
        }
    return report
