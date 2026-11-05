"""One sparse btrfs image holding a project's whole slots tree.

Opt-in via the ``worktrees_image`` configuration section. When configured,
the project's slots directory (``worktrees_dir``) is the mount point of one
sparse btrfs image file that lives outside the mount, on the host file
system. Every slot lives inside the image, so guest metadata (millions of
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

import dataclasses
import os
import subprocess
from pathlib import Path
from typing import Sequence

from wrkslots import slotimage

SCHEMA = "wrkslots-worktrees-image/v1"
MIN_SIZE_BYTES = 1024**3
FILESYSTEMS = ("btrfs",)
MOUNTS = ("kernel", "auto")

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
    """Configuration for the project worktrees image."""

    relative_path: str
    size_bytes: int
    filesystem: str = "btrfs"
    mount: str = "kernel"

    def image_file(self, root: Path) -> Path:
        """The backing image file for a project rooted at ``root``."""

        return root / self.relative_path


#: The comment written above each ``worktrees_image`` key in a literate
#: configuration.
WORKTREES_IMAGE_DOCS: dict[str, str] = {
    "path": (
        "Project-relative backing file for the one sparse btrfs image mounted at the "
        "slots directory. It lives outside the mount, on the host file system."
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
}


def settings_from_obj(value: object) -> WorktreesImageSettings:
    """Validate one ``worktrees_image`` section value (already a mapping)."""

    if not isinstance(value, dict):
        raise WorktreesImageError("configuration.worktrees_image must be a mapping")
    unknown = sorted(set(value) - {"path", "size_bytes", "fs", "mount"})
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
    return WorktreesImageSettings(
        relative_path=candidate.as_posix(), size_bytes=size, filesystem=str(filesystem), mount=str(mount)
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
    """Classify the live slots directory (reads the real mount table)."""

    exists = worktrees.is_dir()
    empty = True
    if exists:
        try:
            empty = next(worktrees.iterdir(), None) is None
        except OSError:
            empty = False
    return classify_mount(
        worktrees, slotimage.mount_table(), slots_dir_exists=exists, slots_dir_empty=empty
    )


def _run(argv: list[str], *, sudo: bool = False) -> subprocess.CompletedProcess[str]:
    command = (["sudo", "-n"] if sudo else []) + argv
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise WorktreesImageError(f"{' '.join(command)} failed: {detail}")
    return result


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


def mount_image(image_file: Path, mount_point: Path) -> None:
    """Mount the image at the slots directory (idempotent)."""

    state = classify_mount(
        mount_point,
        slotimage.mount_table(),
        slots_dir_exists=mount_point.is_dir(),
        slots_dir_empty=(next(mount_point.iterdir(), None) is None if mount_point.is_dir() else True),
    )
    if state == MOUNTED:
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
    _run(["btrfs", "quota", "enable", str(mount_point)], sudo=True)


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


def status_report(root: Path, worktrees: Path, settings: WorktreesImageSettings) -> dict[str, object]:
    """Mount state, apparent size, and host-allocated bytes of the image."""
    image_file = settings.image_file(root)
    report: dict[str, object] = {
        "image_file": str(image_file),
        "mount_point": str(worktrees),
        "configured_size_bytes": settings.size_bytes,
        "state": current_state(root, worktrees, settings),
        "image_exists": image_file.is_file(),
    }
    if image_file.is_file():
        stat = image_file.stat()
        report["apparent_bytes"] = stat.st_size
        report["allocated_bytes"] = stat.st_blocks * 512
    return report
