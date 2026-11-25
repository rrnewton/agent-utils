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
import threading
import dataclasses
import fcntl
import json
import os
import stat
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
class EdenBacking:
    """One configured in-image Eden backing repo (#1804).

    ``name`` is the plain directory name ``create --eden`` selects by;
    ``path`` is the normalized relative suffix under ``.eden-backing/``
    where the repo lives (equal to ``name`` unless a map says otherwise).
    """

    name: str
    path: str


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
    #: In-image Eden backing repos (#1567, #1804): every configured
    #: name/suffix under ``.eden-backing/`` an Eden slot in this
    #: project's image may clone from. None keeps caller-supplied
    #: backing paths (the historical behaviour).
    eden_backing_repos: tuple[EdenBacking, ...] | None = None

    @property
    def eden_backing_repo(self) -> str | None:
        """The sole configured backing name, when exactly one is set."""
        if self.eden_backing_repos is not None and len(self.eden_backing_repos) == 1:
            return self.eden_backing_repos[0].name
        return None

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
        "Optional Eden backing repositories inside the image: one plain name, "
        "a list of names, or a map of name to relative path suffix, each under "
        "<image mount>/.eden-backing/ (its own subvolume outside every slot). "
        "When set, create --eden accepts only a configured name or its resolved "
        "in-image path and refuses anything else, so Eden metadata stays in "
        "the image. Seed each with `wrkslots worktrees-image seed-eden-backing`; "
        "absent keeps caller-supplied backing paths. The list and map forms "
        "need every wrkslots reader on the host to include #1804: older readers "
        "refuse a non-string value."
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
    eden_backings = (
        _parse_eden_backing(eden_backing) if eden_backing is not None else None
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
        eden_backing_repos=eden_backings,
    )


def _eden_backing_name_problem(name: object) -> str | None:
    if (
        not isinstance(name, str)
        or not name
        or name != name.strip()
        or "/" in name
        or name in {".", "..", VALIDATE_SUBDIR}
        or any(char.isspace() for char in name)
    ):
        return (
            "configuration.worktrees_image.eden_backing_repo names must be "
            "plain directory names (no path separators, spaces, or 'validate')"
        )
    return None


def _parse_eden_backing(value: object) -> tuple[EdenBacking, ...]:
    """Validate ``eden_backing_repo``: a name, a list of names, or a map.

    Validated on every host like ``machines``. Names are unique plain
    directory components; a map suffix must be a relative, normalized
    path with no ``..`` that stays under ``.eden-backing/`` (the
    symlink-resolved containment is rechecked when a path is resolved).
    """

    pairs: list[tuple[object, object]] = []
    if isinstance(value, str):
        pairs = [(value, value)]
    elif isinstance(value, list):
        pairs = [(item, item) for item in value]
    elif isinstance(value, dict):
        pairs = list(value.items())
    if not pairs:
        raise WorktreesImageError(
            "configuration.worktrees_image.eden_backing_repo must be one "
            "plain directory name, a non-empty list of names, or a "
            "non-empty map of name to relative path"
        )
    entries: list[EdenBacking] = []
    seen: set[str] = set()
    for name, suffix in pairs:
        problem = _eden_backing_name_problem(name)
        if problem is not None:
            raise WorktreesImageError(problem)
        assert isinstance(name, str)
        if name in seen:
            raise WorktreesImageError(
                "configuration.worktrees_image.eden_backing_repo has a "
                f"duplicate name: {name!r}"
            )
        seen.add(name)
        if (
            not isinstance(suffix, str)
            or not suffix
            or suffix != suffix.strip()
            or "\\" in suffix
        ):
            raise WorktreesImageError(
                "configuration.worktrees_image.eden_backing_repo paths must "
                "be non-empty relative paths under .eden-backing/"
            )
        candidate = Path(suffix)
        parts = candidate.parts
        if (
            candidate.is_absolute()
            or not parts
            or any(part in {"", ".", ".."} for part in parts)
            or parts[0] == VALIDATE_SUBDIR
            or candidate.as_posix() != suffix
        ):
            raise WorktreesImageError(
                "configuration.worktrees_image.eden_backing_repo paths must "
                "be normalized relative paths under .eden-backing/ "
                "(no absolute paths, '..', or 'validate')"
            )
        entries.append(EdenBacking(name=name, path=suffix))
    return tuple(entries)


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


def eden_backing_path(mount_point: Path, backing: EdenBacking) -> Path:
    """The in-image path of one configured backing, containment-checked.

    The configured suffix was validated lexically at load; this rechecks
    after resolving symlinks, so a link inside ``.eden-backing/`` cannot
    point a backing outside it (#1804).
    """

    root = eden_backing_root(mount_point)
    resolved_root = Path(os.path.realpath(root))
    resolved = Path(os.path.realpath(root / backing.path))
    if resolved != resolved_root and resolved_root not in resolved.parents:
        raise WorktreesImageError(
            f"Eden backing {backing.name!r} resolves outside "
            f"{root}: {resolved}"
        )
    return resolved


def hgcache_path(mount_point: Path) -> Path:
    """The in-image remotefilelog cache lazy backings fetch into (#1804)."""

    return mount_point / ".hgcache"


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
    """Mount the image at its configured mount point if it is not mounted.

    The check and the mount hold the bind-records lock, so two
    commands starting together after a boot cannot each see the
    image unmounted and stack two mounts on its mount point (#2178).
    """

    if not settings.direct:
        # Unlocked quick check (#2178): an already-mounted image
        # needs no lock at all; the locked pass below re-reads and
        # re-checks before mounting anything.
        quick_dir = image_mount_dir(control)
        if quick_dir.is_dir() and classify_mount(
            quick_dir,
            slotimage.mount_table(),
            slots_dir_exists=True,
            slots_dir_empty=next(quick_dir.iterdir(), None) is None,
        ) == MOUNTED:
            return
    with _records_lock(control):
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


#: Bound for one privileged (sudo) command. A wedged mount or
#: umount must fail its own command, never freeze every wrkslots
#: command behind the records lock (#2178).
_SUDO_TIMEOUT_SECONDS = 120.0


def _run(argv: list[str], *, sudo: bool = False) -> subprocess.CompletedProcess[str]:
    command = (["sudo", "-n"] if sudo else []) + argv
    try:
        result = subprocess.run(
            command,
            capture_output=True,
            text=True,
            check=False,
            timeout=_SUDO_TIMEOUT_SECONDS if sudo else None,
        )
    except subprocess.TimeoutExpired:
        raise WorktreesImageError(
            f"{' '.join(command)} timed out after {_SUDO_TIMEOUT_SECONDS:g}s"
        ) from None
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


#: How long a command waits for the bind-records lock before
#: refusing. The lock is now held across sudo mounts, so an unbounded
#: wait would let one wedged holder freeze every wrkslots command on
#: the host; a bounded one fails only its own command, and the config
#: load already warns and continues on WorktreesImageError (#2178).
RECORDS_LOCK_TIMEOUT_SECONDS = 120.0


def _flock_records_until(descriptor: int, deadline: float) -> int | None:
    """Acquire the records flock, kernel-blocked until ``deadline``.

    Mirrors cli.py's ``_flock_until`` pattern without importing it
    (cli.py imports this module): try non-blocking first, then let a
    helper thread block in the kernel on a duplicated descriptor and
    hand that descriptor to the caller as the held one. If the
    deadline passes first the wait is cancelled, and a late
    acquisition by the helper is unlocked and closed at once, so it
    never acts and never leaks the lock. Returns the descriptor that
    holds the lock, or None on timeout.
    """

    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return descriptor
    except BlockingIOError:
        pass
    if time.monotonic() >= deadline:
        return None
    try:
        duplicate = os.dup(descriptor)
    except OSError:
        return None
    acquired = threading.Event()
    decision = threading.Lock()
    state: dict[str, int | bool] = {"cancelled": False, "fd": -1}

    def helper() -> None:
        try:
            fcntl.flock(duplicate, fcntl.LOCK_EX)
        except OSError:
            os.close(duplicate)
            acquired.set()
            return
        with decision:
            if state["cancelled"]:
                fcntl.flock(duplicate, fcntl.LOCK_UN)
                os.close(duplicate)
            else:
                state["fd"] = duplicate
        acquired.set()

    threading.Thread(target=helper, daemon=True).start()
    if not acquired.wait(timeout=max(0.0, deadline - time.monotonic())):
        with decision:
            held = state["fd"]
            if isinstance(held, int) and held >= 0:
                # The helper acquired in the gap between its publish
                # and our timeout: the lock is ours, so use it rather
                # than leaking it on the duplicate descriptor.
                return held
            state["cancelled"] = True
        return None
    held = state["fd"]
    return held if isinstance(held, int) and held >= 0 else None


#: Records locks this thread already holds, by lock path. The lock is
#: reentrant for its holder: bind establishment holds it across a
#: whole re-check-and-bind section whose steps themselves update
#: records through ``_update_bind_records`` (#2178).
_RECORDS_HELD = threading.local()


@contextlib.contextmanager
def _records_lock(control: Path) -> Iterator[None]:
    """Hold the one flock that serialises bind-record mutations.

    Every writer reads, modifies and writes under this lock, so two
    concurrent wrkslots processes (migrate, create, a config load
    re-establishing binds) can never drop each other's entries. Bind
    establishment also holds it across its mount-table re-check and
    its one mount, so two config loads cannot each see a checkout
    unmounted and stack identical binds on it (#2178); the work under
    it stays constant-time per slot, with no registry scan.
    """

    control.mkdir(parents=True, exist_ok=True)
    lock_path = control / RECORD_LOCK_NAME
    held: set[Path] = getattr(_RECORDS_HELD, "paths", set())
    _RECORDS_HELD.paths = held
    if lock_path in held:
        yield
        return
    descriptor = os.open(
        lock_path, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600
    )
    deadline = time.monotonic() + RECORDS_LOCK_TIMEOUT_SECONDS
    held_descriptor = _flock_records_until(descriptor, deadline)
    if held_descriptor is None:
        os.close(descriptor)
        raise WorktreesImageError(
            f"bind records lock is busy: {lock_path} was not free within "
            f"{RECORDS_LOCK_TIMEOUT_SECONDS:g}s; another wrkslots command "
            "holds it; retry after it exits"
        )
    held.add(lock_path)
    try:
        yield
    finally:
        held.discard(lock_path)
        fcntl.flock(held_descriptor, fcntl.LOCK_UN)
        os.close(held_descriptor)
        if held_descriptor != descriptor:
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


def record_bind_if_absent(control: Path, slot: str, record: BindRecord) -> bool:
    """Record ``slot``'s bind unless one is recorded; True when added (#2238).

    A locked read-modify-write that touches only ``slot``: a record
    another writer added, changed or dropped for any other slot in the
    meantime is never lost or resurrected, as a whole-map save of an
    earlier unlocked read would.
    """

    added: list[bool] = []

    def add(records: dict[str, BindRecord]) -> dict[str, BindRecord]:
        if slot not in records:
            records[slot] = record
            added.append(True)
        return records

    _update_bind_records(control, add)
    return bool(added)


def restore_bind_records(control: Path, originals: dict[str, BindRecord]) -> list[str]:
    """Put back records a failed multi-slot operation dropped; return them (#2238).

    A locked read-modify-write that touches only the named slots, so
    concurrent changes to every other slot survive. A slot recorded
    again meanwhile keeps its newer record, and a slot whose reclaim
    marker is held is not resurrected: its reclaimer owns it now.
    """

    restored: list[str] = []

    def put_back(records: dict[str, BindRecord]) -> dict[str, BindRecord]:
        for slot, record in sorted(originals.items()):
            if slot in records or reclaim_marker_honoured(control, slot):
                continue
            records[slot] = record
            restored.append(slot)
        return records

    _update_bind_records(control, put_back)
    return restored


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


#: Per-slot reclaim marker (#2238): ``<control>/RECLAIM.<slot>.lock``.
#: A reclaimer (wrkslots' own unbind and remove, or a tool outside
#: wrkslots) holds an exclusive flock on it from before its first
#: unmount of the slot's binds until the slot's bind record is dropped,
#: and ``ensure_binds`` leaves a slot alone while that flock is held.
#: The ``.lock`` suffix is deliberate: every reader, older ones
#: included, counts a top-level ``*.journal`` as registry state (its
#: registry and read identities), so a marker named like a journal
#: would cost concurrent mutations validation rounds; every reader
#: already skips ``*.lock`` names, as for ``DELETION-LOCK.*.lock``.
RECLAIM_MARKER_PREFIX = "RECLAIM."
RECLAIM_MARKER_SUFFIX = ".lock"
RECLAIM_MARKER_KIND = "wrkslots-bind-reclaim"
RECLAIM_MARKER_VERSION = 1

#: Reclaim marker states. ``live``: a reclaimer holds its flock, so the
#: slot's bind is not re-established. ``stale``: nobody holds it (its
#: reclaimer exited), so it is ignored and binds behave as before.
#: ``unverifiable``: not an openable regular file; honoured as live, so
#: a doubt never re-binds a checkout that is being reclaimed.
RECLAIM_LIVE = "live"
RECLAIM_STALE = "stale"
RECLAIM_UNVERIFIABLE = "unverifiable"

#: How long a reclaimer waits for another reclaimer of the same slot
#: before refusing. Readers never wait: they test without blocking.
RECLAIM_MARKER_TIMEOUT_SECONDS = 120.0

#: Re-opens when the marker path stopped naming the locked inode
#: between open and flock (another reclaimer finished and unlinked it).
RECLAIM_MARKER_OPEN_ATTEMPTS = 5


def reclaim_marker_path(control: Path, slot: str) -> Path:
    """The reclaim marker of ``slot`` in ``control`` (#2238)."""

    return control / f"{RECLAIM_MARKER_PREFIX}{slot}{RECLAIM_MARKER_SUFFIX}"


def reclaim_marker_state(control: Path, slot: str) -> str | None:
    """Whether a reclaimer holds ``slot``'s reclaim marker (#2238).

    None when there is no marker. Otherwise the marker is opened
    read-only (never created) and tested with a non-blocking shared
    flock: blocked means its reclaimer is alive (``live``), acquired
    means nobody holds it (``stale``, released at once). Anything that
    is not an openable regular file is ``unverifiable``. Constant time:
    one open and one flock attempt, no directory scan.
    """

    path = reclaim_marker_path(control, slot)
    try:
        descriptor = os.open(
            path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK
        )
    except FileNotFoundError:
        return None
    except OSError:
        return RECLAIM_UNVERIFIABLE
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            return RECLAIM_UNVERIFIABLE
        try:
            fcntl.flock(descriptor, fcntl.LOCK_SH | fcntl.LOCK_NB)
        except BlockingIOError:
            return RECLAIM_LIVE
        except OSError:
            return RECLAIM_UNVERIFIABLE
        fcntl.flock(descriptor, fcntl.LOCK_UN)
        return RECLAIM_STALE
    finally:
        os.close(descriptor)


def reclaim_marker_honoured(control: Path, slot: str) -> bool:
    """Whether bind establishment must leave ``slot`` alone (#2238)."""

    return reclaim_marker_state(control, slot) in (RECLAIM_LIVE, RECLAIM_UNVERIFIABLE)


def reclaim_markers(control: Path) -> dict[str, str]:
    """Every reclaim marker in ``control`` and its state, by slot (status)."""

    try:
        names = sorted(os.listdir(control))
    except OSError:
        return {}
    markers: dict[str, str] = {}
    shortest = len(RECLAIM_MARKER_PREFIX) + len(RECLAIM_MARKER_SUFFIX)
    for name in names:
        if (
            len(name) <= shortest
            or not name.startswith(RECLAIM_MARKER_PREFIX)
            or not name.endswith(RECLAIM_MARKER_SUFFIX)
        ):
            continue
        slot = name[len(RECLAIM_MARKER_PREFIX) : -len(RECLAIM_MARKER_SUFFIX)]
        state = reclaim_marker_state(control, slot)
        if state is not None:
            markers[slot] = state
    return dict(sorted(markers.items()))


class ReclaimMarker:
    """A reclaim marker this process holds; see ``hold_reclaim_marker``."""

    def __init__(self, path: Path, descriptor: int) -> None:
        self.path = path
        self._descriptor = descriptor
        self._unlinked = False

    def unlink(self) -> None:
        """Remove the marker file, still holding its flock (idempotent).

        Only the inode this holder locked is removed: a file that
        replaced it is another reclaimer's and is left in place.
        """

        if self._unlinked:
            return
        self._unlinked = True
        try:
            current = os.lstat(self.path)
        except FileNotFoundError:
            return
        held = os.fstat(self._descriptor)
        if (current.st_dev, current.st_ino) == (held.st_dev, held.st_ino):
            self.path.unlink(missing_ok=True)


#: Reclaim markers this thread holds, by path: taking one again in a
#: nested reclaim reuses it (a second flock on a new descriptor would
#: wait on our own).
_RECLAIMS_HELD = threading.local()


def _acquire_reclaim_marker(path: Path) -> int:
    """Open and exclusively flock ``path``; return the holding descriptor."""

    path.parent.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + RECLAIM_MARKER_TIMEOUT_SECONDS
    for _attempt in range(RECLAIM_MARKER_OPEN_ATTEMPTS):
        try:
            descriptor = os.open(
                path, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW, 0o644
            )
        except OSError as exc:
            raise WorktreesImageError(f"cannot open reclaim marker {path}: {exc}") from exc
        held = _flock_records_until(descriptor, deadline)
        if held is None:
            os.close(descriptor)
            raise WorktreesImageError(
                f"reclaim marker is busy: {path} was not free within "
                f"{RECLAIM_MARKER_TIMEOUT_SECONDS:g}s; another reclaimer of this "
                "slot holds it; retry after it exits"
            )
        if held != descriptor:
            os.close(descriptor)
        try:
            current: os.stat_result | None = os.lstat(path)
        except FileNotFoundError:
            current = None
        locked = os.fstat(held)
        if current is not None and (current.st_dev, current.st_ino) == (
            locked.st_dev,
            locked.st_ino,
        ):
            return held
        # Another reclaimer unlinked the file we opened before we
        # locked it: our lock guards nothing. Open the path again.
        fcntl.flock(held, fcntl.LOCK_UN)
        os.close(held)
    raise WorktreesImageError(
        f"reclaim marker {path} was replaced on each of "
        f"{RECLAIM_MARKER_OPEN_ATTEMPTS} attempts to take it; retry"
    )


@contextlib.contextmanager
def hold_reclaim_marker(control: Path, slot: str, *, writer: str) -> Iterator[ReclaimMarker]:
    """Hold ``slot``'s reclaim marker for one reclaim (#2238).

    Take it BEFORE the first unmount of the slot's binds and drop the
    bind record with ``forget_bind(..., reclaim=marker)``, which
    removes the marker under the records lock in the same step. On any
    other exit (the reclaim was abandoned and the record kept) the
    marker is removed here, still under its flock, so binds behave as
    before. A crashed holder's flock is released by the kernel: its
    file is then stale, ignored by ``ensure_binds``, and taken over by
    the next reclaim of the slot. The JSON payload is diagnostics only;
    readers decide by the flock alone. Lock order: the marker before
    the records lock (readers holding the records lock never wait on
    a marker).
    """

    held: dict[Path, ReclaimMarker] = getattr(_RECLAIMS_HELD, "markers", {})
    _RECLAIMS_HELD.markers = held
    path = reclaim_marker_path(control, slot)
    if path in held:
        yield held[path]
        return
    descriptor = _acquire_reclaim_marker(path)
    marker = ReclaimMarker(path, descriptor)
    held[path] = marker
    try:
        payload = {
            "kind": RECLAIM_MARKER_KIND,
            "version": RECLAIM_MARKER_VERSION,
            "slot": slot,
            "pid": os.getpid(),
            "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "writer": writer,
        }
        data = (json.dumps(payload, sort_keys=True) + "\n").encode("utf-8")
        os.ftruncate(descriptor, 0)
        os.pwrite(descriptor, data, 0)
        yield marker
    finally:
        held.pop(path, None)
        try:
            marker.unlink()
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            os.close(descriptor)


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
    path until it is unmounted, and a retried bind records it. The
    mount and the record happen under the records lock, so a
    concurrent bind of the same slot serialises instead of stacking
    a second bind on the first (#2178).
    """

    with _records_lock(control):
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


def _identical_bind_layers(layers: Sequence[slotimage.MountEntry]) -> bool:
    """Whether every layer is the same bind repeated (#2178).

    Device, root, source, fstype, per-mount options and propagation
    must all match: a deliberate read-only or differently-propagated
    self-bind is not a duplicate and is never collapsed.
    """

    first = layers[0]
    identity = (
        first.device,
        first.root,
        first.source,
        first.fstype,
        first.options,
        first.propagation,
    )
    return all(
        (entry.device, entry.root, entry.source, entry.fstype, entry.options, entry.propagation)
        == identity
        for entry in layers[1:]
    )


def _ordered_stack(
    layers: Sequence[slotimage.MountEntry],
) -> list[slotimage.MountEntry] | None:
    """Stacked layers bottom-first by their mount-id chain, or None.

    In mountinfo each stacked layer's parent ID names the layer
    beneath it. When IDs are missing, duplicated, or the chain is
    not a single line, the order is ambiguous and the caller refuses
    rather than guessing from table order (#2178).
    """

    by_id: dict[int, slotimage.MountEntry] = {}
    for entry in layers:
        if entry.mount_id is None or entry.mount_id in by_id:
            return None
        by_id[entry.mount_id] = entry
    ids = set(by_id)
    bottoms = [entry for entry in layers if entry.parent_id not in ids]
    if len(bottoms) != 1:
        return None
    ordered = [bottoms[0]]
    seen = {bottoms[0].mount_id}
    while len(ordered) < len(layers):
        nxt = [
            entry
            for entry in layers
            if entry.parent_id == ordered[-1].mount_id and entry.mount_id not in seen
        ]
        if len(nxt) != 1:
            return None
        ordered.append(nxt[0])
        seen.add(nxt[0].mount_id)
    return ordered


def doubled_bind_targets(
    control: Path, slots_dir: Path
) -> list[dict[str, object]]:
    """Recorded targets carrying more than one bind layer (read-only)."""

    records = load_bind_records(control)
    try:
        entries = slotimage.mount_table()
    except slotimage.ImageError:
        return []
    doubled: list[dict[str, object]] = []
    for slot in sorted(records):
        record = records[slot]
        target = Path(record.target) if record.target else slots_dir / slot
        layers = [entry for entry in entries if entry.mount_point == target]
        if len(layers) > 1:
            doubled.append(
                {
                    "slot": slot,
                    "target": str(target),
                    "layers": len(layers),
                    "identical": _identical_bind_layers(layers),
                }
            )
    return doubled


def collapse_duplicate_binds(
    control: Path, slots_dir: Path, *, apply: bool
) -> tuple[list[str], list[str]]:
    """Report, and with ``apply`` collapse, identical stacked binds.

    This is the explicit repair for binds stacked by racing config
    loads before the locked re-check landed (#2178); config load
    itself never unmounts. Without ``apply`` nothing changes: each
    doubled target is reported with what would be done. With
    ``apply``, each slot is handled under the records lock with the
    mount table re-read before every unmount; an unmount proceeds
    only while at least two layers are identical (device, root,
    source, fstype, options, propagation) with an unambiguous
    mount-id order, and only the top copy is unmounted (a plain
    umount of the exact mountpoint, never lazy). A busy top layer is
    skipped and reported as busy. The bind record is rewritten only
    from a fresh read. Returns (done, skipped) report lines.
    """

    records = load_bind_records(control)
    done: list[str] = []
    skipped: list[str] = []
    for slot in sorted(records):
        try:
            _collapse_slot(control, slots_dir, records[slot], slot, apply, done, skipped)
        except (WorktreesImageError, slotimage.ImageError) as exc:
            # One slot's failure never hides another slot's progress.
            skipped.append(f"slot {slot}: {exc}")
    return done, skipped


def _collapse_slot(
    control: Path,
    slots_dir: Path,
    record: BindRecord,
    slot: str,
    apply: bool,
    done: list[str],
    skipped: list[str],
) -> None:
    target = Path(record.target) if record.target else slots_dir / slot
    layers = [
        entry for entry in slotimage.mount_table() if entry.mount_point == target
    ]
    if len(layers) < 2:
        return
    if not _identical_bind_layers(layers):
        skipped.append(
            f"slot {slot}: {len(layers)} binds stacked at {target} "
            "differ in device, root, source, fstype, options or "
            "propagation; left alone"
        )
        return
    ordered = _ordered_stack(layers)
    if ordered is None:
        skipped.append(
            f"slot {slot}: cannot order the {len(layers)} binds at "
            f"{target} by mount id; refusing to collapse"
        )
        return
    if not apply:
        done.append(
            f"slot {slot}: {len(layers)} identical binds stacked at "
            f"{target}; would unmount {len(layers) - 1} top "
            f"cop{'y' if len(layers) == 2 else 'ies'}, leaving "
            f"mount id {ordered[0].mount_id}"
        )
        return
    removed = 0
    stop_reason: str | None = None
    with _records_lock(control):
        # At most one unmount per extra layer seen at discovery: an
        # unmount that exits 0 without changing the table must not
        # spin here issuing sudo calls under the lock.
        max_removals = len(layers) - 1
        while removed < max_removals:
            fresh = [
                entry
                for entry in slotimage.mount_table()
                if entry.mount_point == target
            ]
            if len(fresh) < 2:
                break
            if not _identical_bind_layers(fresh):
                stop_reason = "the remaining layers differ; left alone"
                break
            fresh_ordered = _ordered_stack(fresh)
            if fresh_ordered is None:
                stop_reason = "cannot order the remaining layers by mount id; refusing"
                break
            top = fresh_ordered[-1]
            try:
                _run(["umount", str(target)], sudo=True)
            except WorktreesImageError as exc:
                detail = str(exc)
                kind = (
                    "busy"
                    if "busy" in detail.lower()
                    else "not unmountable here"
                )
                stop_reason = (
                    f"top layer (mount id {top.mount_id}) is {kind}: {detail}"
                )
                break
            removed += 1
        remaining = [
            entry
            for entry in slotimage.mount_table()
            if entry.mount_point == target
        ]
        if (
            stop_reason is None
            and removed >= max_removals
            and len(remaining) >= 2
        ):
            stop_reason = "unmounts did not reduce the stack; left alone"
        if removed and not remaining:
            # A concurrent unmount raced ours and took the last
            # layer too. That is an error, never a success, and the
            # record keeps its previous mount id: the next wrkslots
            # command rebinds from it.
            skipped.append(
                f"slot {slot}: NO bind remains at {target} (a "
                "concurrent unmount raced this one); run 'wrkslots "
                "worktrees-image mount' to rebind"
            )
            return
        if removed:
            survivor_id = mount_id_at(target)

            def name_survivor(
                current_records: dict[str, BindRecord],
            ) -> dict[str, BindRecord]:
                current = current_records.get(slot)
                if current is not None:
                    current_records[slot] = BindRecord(
                        subvolume=current.subvolume,
                        mount_id=survivor_id,
                        target=current.target,
                    )
                return current_records

            _update_bind_records(control, name_survivor)
            if len(remaining) == 1:
                done.append(
                    f"slot {slot}: removed {removed} duplicate "
                    f"bind(s) at {target}; surviving mount id "
                    f"{survivor_id}"
                )
            else:
                done.append(
                    f"slot {slot}: removed {removed} duplicate "
                    f"bind(s) at {target}; {len(remaining)} layers "
                    f"remain, top mount id {survivor_id}"
                )
    if stop_reason is not None and removed == 0:
        skipped.append(f"slot {slot}: {stop_reason}")
    elif stop_reason is not None:
        skipped.append(
            f"slot {slot}: stopped after {removed} removal(s): {stop_reason}"
        )


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


def forget_bind(control: Path, slot: str, *, reclaim: ReclaimMarker | None = None) -> None:
    """Drop a slot's bind record only; mounts are the caller's business.

    With ``reclaim`` (the slot's held reclaim marker, #2238) the marker
    is removed under the same records lock, right after the record is
    dropped, so a bind establishment that holds the lock sees either
    the marker or no record, never a record without its marker.
    """

    def drop(records: dict[str, BindRecord]) -> dict[str, BindRecord]:
        records.pop(slot, None)
        return records

    with _records_lock(control):
        _update_bind_records(control, drop)
        if reclaim is not None:
            reclaim.unlink()


def umount_point(path: Path) -> None:
    """Unmount the bind at ``path``; the bind record is untouched."""

    unmount_bind(path)


def unbind_slot(control: Path, slot: str, slot_path: Path) -> None:
    """Unmount a slot's bind and drop its record (original stays beneath).

    The slot's reclaim marker is held from before the unmount until the
    record is dropped (#2238), so no wrkslots command anywhere re-binds
    the slot in between; a failed unmount removes the marker and keeps
    the record. Crash order (unmount, then record): a crash between
    them leaves a record saying bound with nothing mounted, the
    subvolume intact and a stale marker, and the next config load's
    ``ensure_binds`` re-binds it, so a remove interrupted here retries
    from the bound state. A record that outlives its subvolume (deleted
    outside this order) is skipped by ``ensure_binds``, never re-created
    and never a load failure.
    """

    with hold_reclaim_marker(control, slot, writer="wrkslots unbind") as reclaim:
        unmount_bind(slot_path)
        forget_bind(control, slot, reclaim=reclaim)


def _binds_need_locked_pass(
    control: Path,
    mount_dir: Path,
    slots_dir: Path,
    records: dict[str, BindRecord],
    entries: Sequence[slotimage.MountEntry],
) -> bool:
    """Whether the locked re-check could have anything to do (#2178).

    Mirrors the locked loop's skips from an unlocked read: journaled
    and fenced slots are left alone either way, a mounted target
    needs the lock only while its top is shared, an unmounted target
    needs it only while its subvolume still exists, and a slot whose
    reclaim marker is held needs nothing (#2238). The marker is tested
    only for a slot that would otherwise need work, so the common
    all-mounted load costs no extra system call.
    """

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
                if reclaim_marker_honoured(control, slot):
                    continue
                return True
            continue
        if (mount_dir / record.subvolume).is_dir():
            if reclaim_marker_honoured(control, slot):
                continue
            return True
    return False


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
    removal that journaled it (#1733). A slot whose reclaim marker is held
    (``RECLAIM.<slot>.lock``, #2238) is left alone too: a reclaimer has
    unmounted, or is unmounting, its binds and drops its record next,
    and a bind made here would pin the checkout it is removing.
    The marker is tested after the mount table is read under the lock,
    and every reclaimer takes it before its first unmount, so a target
    found unmounted by a live reclaimer always finds its marker held; a
    stale marker (its reclaimer exited) is ignored. A bind that is
    already mounted but still shared is made private here, so existing
    binds migrate without an operator step.

    The direct layout has no binds at all: with direct ``settings`` this
    returns before any record is read or written.
    """

    if settings is not None and settings.direct:
        return []
    actions: list[str] = []
    if not mount_dir.is_dir() or next(mount_dir.iterdir(), None) is None:
        pass
    # Double-checked locking (#2178): most config loads, read-only
    # ones included, find the image mounted and every bind already
    # in place. That case is decided from an unlocked read and never
    # waits on the records lock; the lock is taken, and everything
    # re-read inside it, only when a bind or a privatise is missing.
    unlocked_records = load_bind_records(control)
    unlocked_entries = slotimage.mount_table()
    if not _binds_need_locked_pass(
        control, mount_dir, slots_dir, unlocked_records, unlocked_entries
    ):
        return []
    # The re-check-and-bind runs under the records lock (#2178):
    # the mount table and the records are read again inside it, so a
    # concurrent config load that bound a slot first is seen, not
    # raced. Each slot's work under the lock is constant-time (one
    # table, one mount at most), never a registry scan. Every binding
    # decision comes from this locked pass alone (#2238, a consumer's
    # binding requirement): it iterates the records loaded under this
    # lock hold, never the unlocked pre-read above (which may only
    # skip work), so a reclaimer that popped a record and unlinked its
    # marker just before this lock is never re-bound from a stale
    # list; and the reclaim-marker probe sits per slot, after this
    # pass's one mount-table snapshot, right before any bind.
    with _records_lock(control):
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
                    if reclaim_marker_honoured(control, slot):
                        continue
                    make_private(target, entries)
                    actions.append(f"made slot {slot} bind private")
                continue
            if not (mount_dir / record.subvolume).is_dir():
                # The record outlived its subvolume (a remove deleted the
                # subvolume outside unbind's crash order). Skip it: do not
                # fail every config load, do not re-create the slot path or
                # subvolume, and do not block the remaining binds.
                continue
            if reclaim_marker_honoured(control, slot):
                # A reclaimer holds the slot (#2238): it unmounted the
                # bind and drops the record next, under this same lock.
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
        "doubled_binds": (
            []
            if settings.direct
            else doubled_bind_targets(worktrees.parent, worktrees)
        ),
        "reclaim_markers": {} if settings.direct else reclaim_markers(worktrees.parent),
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
    if settings.eden_backing_repos:
        from wrkslots import eden

        entries: list[dict[str, object]] = []
        for backing in settings.eden_backing_repos:
            try:
                backing_path: Path | None = eden_backing_path(mount_point, backing)
            except WorktreesImageError:
                backing_path = None
            problem = (
                eden.backing_repo_problem(backing_path)
                if state == MOUNTED and backing_path is not None
                else None
            )
            entries.append(
                {
                    "configured_name": backing.name,
                    "path": str(backing_path) if backing_path is not None else "",
                    "present": (
                        backing_path.is_dir()
                        if state == MOUNTED and backing_path is not None
                        else False
                    ),
                    "repo_ok": problem is None and state == MOUNTED and backing_path is not None,
                }
            )
        report["eden_backing"] = entries[0] if len(entries) == 1 else entries
    return report
